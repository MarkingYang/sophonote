"""Exercise the embedded Hermes boundary without desktop access or permissions."""
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class ProxyContract(unittest.TestCase):
    def setUp(self):
        directory = ROOT / 'src-tauri' / 'target'
        directory.mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix='cua-proxy-test-', dir=directory)
        self.root = Path(self.temp.name)
        self.proxy = self.root / 'hermes-cua'
        shutil.copyfile(ROOT / 'scripts/assets/hermes-cua', self.proxy)
        self.proxy.chmod(0o755)
        driver = self.root / 'cua-driver'
        driver.write_text('#!/bin/sh\nprintf "args=%s\\n" "$*"\nprintf "secret=%s\\n" "${OPENAI_API_KEY:-absent}"\nprintf "mode=%s\\n" "${CUA_DRIVER_PERMISSION_MODE:-absent}"\n')
        driver.chmod(0o755)
        # macOS sockets have a short path limit; use an abstract test of the
        # wrapper's real socket check via a relative path in the fixture cwd.
        self.socket = socket.socket(socket.AF_UNIX)
        old = Path.cwd()
        try:
            os.chdir(self.root)
            self.socket.bind('driver.sock')
        finally:
            os.chdir(old)
        self.env = {**os.environ, 'SOPHONOTE_CUA_SOCKET': 'driver.sock',
                    'SOPHONOTE_CUA_HOME': str(self.root), 'OPENAI_API_KEY': 'test-secret',
                    'CUA_DRIVER_PERMISSION_MODE': 'unrestricted'}

    def tearDown(self):
        self.socket.close()
        self.temp.cleanup()

    def run_proxy(self, *args):
        return subprocess.run([str(self.proxy), *args], cwd=self.root, env=self.env,
                              text=True, capture_output=True, timeout=5)

    def test_manifest_keeps_hermes_on_the_proxy(self):
        invocation = json.loads(self.run_proxy('manifest').stdout)['mcp_invocation']
        self.assertEqual(invocation, {'command': 'cua-driver', 'args': ['mcp']})

    def test_proxy_has_private_endpoint_and_no_provider_keys_or_mode_override(self):
        result = self.run_proxy('mcp')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('mcp --embedded --socket driver.sock', result.stdout)
        self.assertIn('secret=absent', result.stdout)
        self.assertIn('mode=absent', result.stdout)

    def test_no_standalone_start_update_or_arbitrary_endpoint(self):
        for args in [('serve',), ('update', '--apply'), ('permissions', 'grant'),
                     ('mcp', '--direct'), ('mcp', '--socket', '/tmp/other.sock')]:
            with self.subTest(args=args):
                self.assertNotEqual(self.run_proxy(*args).returncode, 0)

    def test_missing_daemon_never_falls_back(self):
        self.env['SOPHONOTE_CUA_SOCKET'] = 'missing.sock'
        self.assertNotEqual(self.run_proxy('mcp').returncode, 0)


if __name__ == '__main__':
    unittest.main()
