"""Bundled Hermes loop against a local synthetic model and Host RPC fixture.

No cloud credentials, user config, or real tools are used. This proves native
tool dispatch reaches the Host wire; Rust tests prove approval enforcement.
Run with the bundled Python and its site-packages on PYTHONPATH.
"""
import json
import os
from pathlib import Path
import tempfile
import threading
import sys
import time
from unittest.mock import patch
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[2]
ROOT.joinpath("src-tauri/target").mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix="hermes-host-native-", dir=ROOT / "src-tauri/target") as home:
    os.environ.update(HERMES_HOME=home, HOME=home, SOPHONOTE_HOST_TOOLS_REQUIRED="1",
                      HERMES_DISABLE_BACKGROUND_MAINTENANCE="1", NO_PROXY="127.0.0.1,localhost")
    Path(home, "config.yaml").write_text("memory:\n  memory_enabled: false\n  user_profile_enabled: false\n")
    requests = []
    auxiliary_requests = []
    tool_calls = []
    failures = []
    lease = "synthetic-native-host-lease"

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args): pass

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if self.path == "/api/show":
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", "2")
                self.end_headers()
                self.wfile.write(b"{}")
                return
            if self.path != "/mcp" and "messages" not in body:
                failures.append("unexpected fixture request " + self.path + " keys=" + str(list(body)))
                self.send_error(400)
                return
            if self.path == "/mcp":
                if self.headers.get("Authorization") != "Bearer " + lease:
                    self.send_error(403)
                    return
                if body["method"] == "tools/list":
                    result = {"tools": [{"name": "write", "description": "Host controlled write",
                        "inputSchema": {"type": "object", "properties": {"path": {"type": "string"},
                        "content": {"type": "string"}}, "required": ["path", "content"]}}]}
                else:
                    tool_calls.append(body["params"])
                    result = {"isError": True, "content": [{"type": "text", "text": "HOST_DENIED_FIXTURE"}]}
                payload = {"jsonrpc": "2.0", "id": body["id"], "result": result}
            else:
                auxiliary = not body.get("tools") and not body.get("stream")
                (auxiliary_requests if auxiliary else requests).append(body)
                names = [t["function"]["name"] for t in body.get("tools", [])]
                if not auxiliary and names != ["write"]: failures.append("model received unexpected tools: " + str(names))
                if lease in json.dumps(body): failures.append("lease reached model")
                if auxiliary or body["messages"][-1].get("role") == "tool":
                    message = {"role": "assistant", "content": "HOST_NATIVE_FINISHED"}
                    reason = "stop"
                else:
                    message = {"role": "assistant", "content": None, "tool_calls": [{"id": f"fixture-call-{len(requests)}",
                        "type": "function", "function": {"name": "write", "arguments": json.dumps({
                            "path": str(Path(home, "must-not-exist.txt")), "content": "NO_WRITE"})}}]}
                    reason = "tool_calls"
                payload = {"id": "synthetic-completion", "object": "chat.completion", "created": 1,
                    "model": "host-fixture", "choices": [{"index": 0, "message": message, "finish_reason": reason}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}}
                if body.get("stream"):
                    delta = dict(message)
                    for index, call in enumerate(delta.get("tool_calls", [])): call["index"] = index
                    payload["object"] = "chat.completion.chunk"
                    payload["choices"] = [{"index": 0, "delta": delta, "finish_reason": reason}]
                    data = ("data: " + json.dumps(payload) + "\n\ndata: [DONE]\n\n").encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)
                    return
            data = json.dumps(payload).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    base = f"http://127.0.0.1:{server.server_port}"
    try:
        from run_agent import AIAgent
        from hermes_host_tools import install
        def make_agent(*args, **kwargs):
            return AIAgent(base_url=base + "/v1", api_key="synthetic-model-key", provider="custom",
                           api_mode="chat_completions", model="host-fixture", max_iterations=3,
                           enabled_toolsets=[], skip_memory=True, skip_context_files=True,
                           quiet_mode=True, save_trajectories=False)

        turns = 1
        if "--gateway" in sys.argv:
            from tui_gateway import server as gateway
            events = []
            # Only construction/configuration is replaced: the production session
            # handlers, install timing, turn runner, callbacks and model wire run.
            with patch.object(gateway, "_make_agent", side_effect=make_agent), patch.object(
                    gateway, "_emit", side_effect=lambda event, sid, payload=None: events.append((event, payload))):
                created = gateway._methods["session.create"](1, {
                    "model": "host-fixture", "provider": "custom", "cwd": home})
                assert "error" not in created, created
                sid = created["result"]["session_id"]
                # Missing Host authorization must fail before the model is called.
                denied = gateway._methods["prompt.submit"](2, {"session_id": sid, "text": "no lease"})
                assert denied.get("error", {}).get("code") == 4030, denied
                assert not requests and not tool_calls
                turns = 2
                for turn in range(turns):
                    submitted = gateway._methods["prompt.submit"](3 + turn, {
                        "session_id": sid, "text": "Use the provided write tool once, then report its denial.",
                        "sophonote_host_tools": {"version": 1, "url": base + "/mcp", "bearer": lease}})
                    assert "error" not in submitted, submitted
                    deadline = time.monotonic() + 25
                    while gateway._sessions[sid]["running"] and time.monotonic() < deadline:
                        threading.Event().wait(.02)
                    assert not gateway._sessions[sid]["running"], "Gateway turn did not finish"
                errors = [payload for event, payload in events if event == "error"]
                assert not errors, errors
                assert sum(event == "message.complete" for event, _ in events) == turns, events
                result = gateway._sessions[sid]["history"]
        else:
            agent = make_agent()
            install(agent, {"version": 1, "url": base + "/mcp", "bearer": lease})
            # Real per-turn MCP refresh must not replace the Host tool surface.
            from tools.mcp_tool import refresh_agent_mcp_tools
            refresh_agent_mcp_tools(agent, enabled_override=["terminal", "file"])
            assert agent.valid_tool_names == {"write"}, sorted(agent.valid_tool_names)
            result = agent.run_conversation("Use the provided write tool once, then report its denial.")
        assert not failures, {"failures": failures, "requests": [{"tools": [t["function"]["name"] for t in r.get("tools", [])], "roles": [m.get("role") for m in r["messages"]], "max_tokens": r.get("max_tokens"), "stream": r.get("stream"), "tool_choice": r.get("tool_choice")} for r in requests]}
        assert len(tool_calls) == turns, tool_calls
        assert tool_calls[0]["name"] == "write", tool_calls
        assert len(requests) == 2 * turns, len(requests)
        assert "HOST_DENIED_FIXTURE" in json.dumps(requests[-1]["messages"])
        assert "HOST_NATIVE_FINISHED" in json.dumps(result)
        assert not Path(home, "must-not-exist.txt").exists()
        print(f"PASS: bundled {'Gateway' if turns == 2 else 'native loop'} -> authenticated Host RPC -> denial -> model result ({turns} turns); no native write")
    finally:
        server.shutdown()
        server.server_close()
        worker.join()
