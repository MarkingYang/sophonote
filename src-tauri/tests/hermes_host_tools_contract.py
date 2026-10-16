"""Exercise the pinned native executors, without a model or user credentials."""
import ast
import importlib.util
import os
from pathlib import Path
import sys
import types
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/assets"))
import hermes_host_tools as policy


class HostToolContract(unittest.TestCase):
    def test_policy_rejects_non_loopback_and_redirect_targets(self):
        for url in ("https://127.0.0.1:1/mcp", "http://evil.test:1/mcp", "http://127.0.0.1:1/mcp?token=x", "http://u:p@127.0.0.1:1/mcp"):
            with self.assertRaises(ValueError):
                policy.checked_policy({"version": 1, "url": url, "bearer": "synthetic-token-123"})

    def test_policy_is_a_copy_and_never_accepts_missing_version(self):
        original = {"version": 1, "url": "http://127.0.0.1:1234/mcp", "bearer": "synthetic-token-123"}
        self.assertIsNot(policy.checked_policy(original), original)
        with self.assertRaises(ValueError): policy.checked_policy(None)

    def test_both_native_dispatch_paths_never_reach_native_body(self):
        source = ROOT / "src-tauri/resources/hermes/aarch64-apple-darwin/runtime/site-packages/agent/tool_executor.py"
        tree = ast.parse(source.read_text())
        fake = types.ModuleType("agent.tool_executor")
        fake.make_tool_result_message = lambda name, text, call_id: {"name": name, "content": text, "id": call_id}
        fake._flush_session_db_after_tool_progress = lambda *a, **kw: True
        call = types.SimpleNamespace(id="call-fixture", function=types.SimpleNamespace(name="write_file", arguments='{"path":"forbidden","content":"no"}'))
        for name in ("execute_tool_calls_sequential", "execute_tool_calls_concurrent"):
            function = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == name)
            namespace = {}
            exec(compile(ast.Module(body=[function], type_ignores=[]), str(source), "exec"), namespace)
            with self.subTest(path=name), patch.dict(os.environ, {"SOPHONOTE_HOST_TOOLS_REQUIRED": "1"}), patch.dict(sys.modules, {"agent.tool_executor": fake}):
                messages = []
                namespace[name](types.SimpleNamespace(), types.SimpleNamespace(tool_calls=[call]), messages, "fixture")
                self.assertEqual(len(messages), 1)
                self.assertIn("native execution denied", messages[0]["content"])

    def test_cancel_does_not_contact_host(self):
        fake = types.ModuleType("agent.tool_executor")
        fake.make_tool_result_message = lambda *a: a
        fake._flush_session_db_after_tool_progress = lambda *a, **kw: True
        call = types.SimpleNamespace(id="c", function=types.SimpleNamespace(name="write", arguments='{}'))
        with patch.dict(os.environ, {"SOPHONOTE_HOST_TOOLS_REQUIRED": "1"}), patch.dict(sys.modules, {"agent.tool_executor": fake}), patch.object(policy, "rpc") as rpc:
            self.assertTrue(policy.execute_batch(types.SimpleNamespace(_interrupt_requested=True), types.SimpleNamespace(tool_calls=[call]), [], "fixture"))
            rpc.assert_not_called()

    def test_install_exposes_only_host_tools(self):
        lease = {"version": 1, "url": "http://127.0.0.1:1234/mcp", "bearer": "synthetic-token-123"}
        agent = types.SimpleNamespace(tools=[{"name": "delegate_task"}])
        with patch.object(policy, "rpc", return_value={"tools":[{"name":"read", "description":"read", "inputSchema":{}}]}):
            policy.install(agent, lease)
        self.assertEqual(agent.valid_tool_names, {"read"})
        self.assertNotIn("bearer", str(agent.tools))


if __name__ == "__main__": unittest.main()
