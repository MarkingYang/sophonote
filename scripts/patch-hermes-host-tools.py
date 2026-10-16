#!/usr/bin/env python3
"""Fail-closed compatibility patch for the pinned Hermes execution entrypoints."""
from pathlib import Path
import shutil
import sys


def replace_once(path, anchor, replacement):
    text = path.read_text()
    if replacement in text:
        return
    if text.count(anchor) != 1:
        raise RuntimeError(f"Pinned Hermes Host tools patch drift: {path.name}")
    path.write_text(text.replace(anchor, replacement))


def patch(root):
    # The Gateway's per-turn MCP refresh otherwise replaces the Host snapshot
    # with native tool_search/tool_call after install() has already completed.
    target = root / "tools/mcp_tool.py"
    text = target.read_text()
    start = text.index("def refresh_agent_mcp_tools(")
    end = text.index('\n    """', start) + 8
    end = text.index('"""', end) + 3
    anchor = text[start:end]
    replace_once(target, anchor, anchor + '\n    if getattr(agent, "_sophonote_host_tools", None) is not None:\n        return set()  # Host owns this Run\'s complete tool snapshot.\n')
    anchor = '    with _agent_tools_lock:\n        # Defensive: the published generation'
    replace_once(target, anchor, '    with _agent_tools_lock:\n        if getattr(agent, "_sophonote_host_tools", None) is not None:\n            return set()\n        # Defensive: the published generation')
    target = root / "agent/tool_executor.py"
    for name in ("concurrent", "sequential"):
        prefix = f"def execute_tool_calls_{name}("
        text = target.read_text()
        start = text.index(prefix)
        end = text.index('\n    """', start) + 8
        end = text.index('"""', end) + 3
        anchor = text[start:end]
        marker = "\n    from hermes_host_tools import execute_batch\n    if execute_batch(agent, assistant_message, messages, effective_task_id):\n        return\n"
        replace_once(target, anchor, anchor + marker)
    target = root / "tui_gateway/methods_prompt.py"
    anchor = '    isolation_cfg = _load_dashboard_process_isolation_config()\n'
    replace_once(target, anchor, '''    from hermes_host_tools import checked_policy
    host_policy = None
    if os.environ.get("SOPHONOTE_HOST_TOOLS_REQUIRED") == "1":
        try:
            host_policy = checked_policy(params.get("sophonote_host_tools"))
        except Exception:
            return _err(rid, 4030, "Host tool authorization required")
''' + anchor)
    anchor = '    turn_isolation = _session_uses_compute_host(session, isolation_cfg)\n'
    replace_once(target, anchor, anchor + '    if host_policy is not None:\n        turn_isolation = False  # Run lease is process-local; never copy it to another worker.\n')
    anchor = '        _run_prompt_submit(rid, sid, session, text)\n'
    replace_once(target, anchor, '''        if host_policy is not None:
            try:
                from hermes_host_tools import install
                install(session["agent"], host_policy)
            except Exception:
                _emit_terminal_turn_error(sid, session, "Host tool authorization failed")
                with session["history_lock"]:
                    session["running"] = False
                    _clear_inflight_turn(session)
                return
''' + anchor)
    anchor = '@method("prompt.submit")\n'
    replace_once(target, anchor, '''@method("sophonote.host_tools")
def _sophonote_host_tools(rid, params):
    import os
    return _ok(rid, {"version": 1, "required": os.environ.get("SOPHONOTE_HOST_TOOLS_REQUIRED") == "1"})


''' + anchor)
    shutil.copyfile(Path(__file__).parent / "assets/hermes_host_tools.py", root / "hermes_host_tools.py")


if __name__ == "__main__":
    patch(Path(sys.argv[1]))
