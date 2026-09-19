#!/usr/bin/env python3
"""Audited, fail-closed compatibility delta for the pinned Hermes cloud provider.

The official managed endpoint requires authentication even on /health. Other
endpoints keep upstream's anonymous identity gate. Run before bundle hashing.
"""
from pathlib import Path
import sys

MARKER = '    # SophoNote: managed Volcengine requires authentication on /health.\n'
ANCHOR = '    """Classify runtime health without treating every false result as server absence."""\n'
PATCH = '''    # SophoNote: managed Volcengine requires authentication on /health.
    if endpoint.rstrip("/") == "https://api.vikingdb.cn-beijing.volces.com/openviking":
        if not client._api_key:
            return "responded", "Cloud OpenViking API Key is not configured."
        try:
            response = client.get("/api/v1/fs/ls", params={"uri": "viking://~", "limit": 1})
            if response.get("status") == "ok" and isinstance(response.get("result"), list):
                return "healthy", ""
        except Exception:
            pass
        return "responded", "Cloud OpenViking authenticated directory check failed."
'''


def patch(path: Path) -> None:
    content = path.read_text(encoding='utf-8')
    if MARKER in content:
        if PATCH not in content:
            raise RuntimeError('Cloud provider patch drift; inspect before bundling')
        return
    if content.count(ANCHOR) != 1:
        raise RuntimeError('Pinned Hermes health adapter changed; inspect before bundling')
    path.write_text(content.replace(ANCHOR, ANCHOR + PATCH), encoding='utf-8')


if __name__ == '__main__':
    patch(Path(sys.argv[1]) / 'plugins/memory/openviking/__init__.py')
