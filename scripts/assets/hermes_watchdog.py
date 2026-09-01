#!/usr/bin/env python3
"""Watchdog between SophoNote and bundled Hermes.

Unix still ships the POSIX `hermes-launcher`. Windows uses this module as the
Manifest launcher so the Host can spawn `python.exe` without a flashing
console, while still reaping Hermes if the parent process disappears.
"""
from __future__ import annotations

import os
import subprocess
import sys
import time
from pathlib import Path


def _runtime_root() -> Path:
    return Path(__file__).resolve().parent.parent


def _ensure_python_env() -> None:
    runtime = _runtime_root()
    python_home = runtime / "python"
    site = runtime / "site-packages"
    os.environ.setdefault("PYTHONHOME", str(python_home))
    os.environ.setdefault("PYTHONPATH", str(site))
    os.environ.setdefault("PYTHONDONTWRITEBYTECODE", "1")


def host_alive(pid: int) -> bool:
    if pid <= 0:
        return True
    if os.name == "nt":
        import ctypes

        kernel32 = ctypes.windll.kernel32
        process_query_limited_information = 0x1000
        still_active = 259
        handle = kernel32.OpenProcess(process_query_limited_information, False, pid)
        if not handle:
            return False
        try:
            exit_code = ctypes.c_ulong()
            if kernel32.GetExitCodeProcess(handle, ctypes.byref(exit_code)) == 0:
                return False
            return int(exit_code.value) == still_active
        finally:
            kernel32.CloseHandle(handle)
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def _stop(child: subprocess.Popen[bytes]) -> None:
    if child.poll() is not None:
        return
    child.terminate()
    try:
        child.wait(timeout=5)
    except subprocess.TimeoutExpired:
        child.kill()
        child.wait()


def main() -> int:
    _ensure_python_env()
    host_pid = int(os.environ.get("SOPHONOTE_HOST_PID") or "0")
    child = subprocess.Popen(
        [sys.executable, "-m", "hermes_cli.main", *sys.argv[1:]],
        close_fds=os.name != "nt",
    )
    try:
        while child.poll() is None:
            if not host_alive(host_pid):
                _stop(child)
                return 0
            time.sleep(1)
        return child.returncode or 0
    except KeyboardInterrupt:
        _stop(child)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
