"""Run-scoped tool delegation. Never fall back to native execution on failure."""
import json
import os
import sys
import urllib.request
from contextlib import nullcontext
from urllib.parse import urlsplit

VERSION = 1


def checked_policy(value):
    if not isinstance(value, dict) or value.get("version") != VERSION:
        raise ValueError("SophoNote Host tool authorization is required")
    url = urlsplit(value.get("url", ""))
    token = value.get("bearer")
    if (url.scheme != "http" or url.hostname != "127.0.0.1" or not url.port
            or url.path != "/mcp" or url.username or url.password or url.query or url.fragment
            or not isinstance(token, str) or not 16 <= len(token) <= 256):
        raise ValueError("Invalid SophoNote Host tool authorization")
    return dict(value)


def rpc(policy, method, params):
    policy = checked_policy(policy)
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    if len(body) > 2 * 1024 * 1024:
        raise ValueError("Tool request is too large")
    request = urllib.request.Request(policy["url"], body, {
        "Authorization": "Bearer " + policy["bearer"], "Content-Type": "application/json"})
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args, **kwargs):
            return None
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    with opener.open(request, timeout=610) as response:
        data = response.read(2 * 1024 * 1024 + 1)
    if len(data) > 2 * 1024 * 1024:
        raise ValueError("Tool response is too large")
    frame = json.loads(data)
    if frame.get("error") or "result" not in frame:
        raise ValueError("Host tool protocol failed")
    return frame["result"]


def install(agent, policy):
    policy = checked_policy(policy)
    definitions = rpc(policy, "tools/list", {})["tools"]
    # Expose only the Host's tool surface; old native history cannot grant tools.
    tools = [{"type": "function", "function": {
        "name": t["name"], "description": t["description"], "parameters": t["inputSchema"]}}
        for t in definitions]
    # Serialize with an already-running late MCP refresh's atomic publish.
    lock = getattr(sys.modules.get("tools.mcp_tool"), "_agent_tools_lock", None)
    with lock if lock is not None else nullcontext():
        agent._sophonote_host_tools = policy
        agent.tools = tools
        agent.valid_tool_names = {t["name"] for t in definitions}


def execute_batch(agent, assistant_message, messages, effective_task_id):
    if os.environ.get("SOPHONOTE_HOST_TOOLS_REQUIRED") != "1":
        return False
    from agent.tool_executor import make_tool_result_message, _flush_session_db_after_tool_progress
    for call in assistant_message.tool_calls:
        name = call.function.name
        result = {"error": "Host tool authorization unavailable; native execution denied"}
        try:
            if getattr(agent, "_interrupt_requested", False):
                raise ValueError("cancelled")
            args = json.loads(call.function.arguments)
            if not isinstance(args, dict):
                raise ValueError("Invalid arguments")
            # No native-name aliases or tool_search unwrap: unknown tools fail closed in Rust.
            result = rpc(getattr(agent, "_sophonote_host_tools", None), "tools/call", {
                "name": name, "arguments": args})
        except Exception:
            # Do not expose the loopback lease or credentials through exception strings.
            pass
        text = json.dumps(result, ensure_ascii=False)
        messages.append(make_tool_result_message(name, text, call.id))
        if not _flush_session_db_after_tool_progress(agent, messages, stage="Host tool result"):
            break
    return True
