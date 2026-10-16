#!/usr/bin/env python3
"""Native Pi request-budget contract. Loopback model only; never reads credentials."""
import json
import os
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[2]
RUNTIME = ROOT / 'src-tauri/resources/pi/aarch64-apple-darwin'

class BudgetContract(unittest.TestCase):
    def exercise(self, limit):
        requests = []
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass
            def do_POST(self):
                request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                requests.append(request)
                index = len(requests)
                delta = {'tool_calls': [{'index': 0, 'id': f'read-{index}', 'type': 'function',
                         'function': {'name': 'read', 'arguments': '{"path":"fixture.txt"}'}}]}
                def chunk(part, finish):
                    return {'id': f'fixture-{index}', 'object': 'chat.completion.chunk',
                            'created': 0, 'model': 'fixture', 'choices': [{'index': 0, 'delta': part, 'finish_reason': finish}]}
                body = ('data: ' + json.dumps(chunk(delta, None)) + '\n\ndata: ' +
                        json.dumps(chunk({}, 'tool_calls')) + '\n\ndata: [DONE]\n\n').encode()
                self.send_response(200)
                self.send_header('Content-Type', 'text/event-stream')
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        serving = threading.Thread(target=server.serve_forever, daemon=True)
        serving.start()
        output = ROOT / 'src-tauri/target/pi-budget-contract'
        output.mkdir(parents=True, exist_ok=True)
        try:
            with tempfile.TemporaryDirectory(dir=output) as temporary:
                env = {'HOME': temporary, 'PI_CODING_AGENT_DIR': temporary,
                       'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'LANG': 'en_US.UTF-8',
                       'PI_OFFLINE': '1', 'SOPHONOTE_PI_PARENT': str(os.getpid()),
                       'NO_PROXY': '127.0.0.1,localhost',
                       'SOPHONOTE_PI_MODEL_KEY': 'local-fixture',
                       'SOPHONOTE_PI_MODEL': json.dumps({'baseUrl': f'http://127.0.0.1:{server.server_port}/v1',
                                                       'id': 'fixture', 'api': 'openai-completions'})}
                if limit is not None:
                    env['SOPHONOTE_PI_MAX_REQUESTS'] = str(limit)
                process = subprocess.Popen([str(RUNTIME / 'pi/pi'), '--mode', 'rpc', '--offline',
                    '--no-builtin-tools', '--no-extensions', '--no-skills', '--no-prompt-templates',
                    '--no-themes', '--no-context-files', '--no-approve', '--extension', str(ROOT / 'scripts/assets/pi-policy.ts'),
                    '--provider', 'sophonote', '--model', 'fixture', '--session', str(Path(temporary) / 'session.jsonl')],
                    cwd=temporary, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                    stderr=subprocess.DEVNULL, text=True)
                frames = queue.Queue()
                def read_frames():
                    for line in process.stdout:
                        frames.put(json.loads(line))
                    frames.put(None)
                reader = threading.Thread(target=read_frames, daemon=True)
                reader.start()
                def send(value):
                    process.stdin.write(json.dumps(value) + '\n')
                    process.stdin.flush()
                try:
                    send({'id': 'prompt', 'type': 'prompt', 'message': 'Keep reading fixture.txt.'})
                    deadline = time.monotonic() + 20
                    while True:
                        remaining = deadline - time.monotonic()
                        self.assertGreater(remaining, 0, "native budget did not terminate the loop")
                        frame = frames.get(timeout=remaining)
                        if frame is None:
                            break
                        if frame.get('type') == 'extension_ui_request':
                            self.assertEqual(frame.get('title'), 'sophonote.tool')
                            send({'type': 'extension_ui_response', 'id': frame['id'],
                                  'value': json.dumps({'ok': True, 'text': 'synthetic fixture evidence'})})
                    self.assertEqual(process.wait(timeout=3), 77)
                    self.assertEqual(len(requests), limit or 0)
                finally:
                    if process.poll() is None:
                        process.kill()
                    process.wait(timeout=5)
                    process.stdin.close()
                    process.stdout.close()
                    reader.join(timeout=2)
        finally:
            server.shutdown()
            server.server_close()
            serving.join(timeout=2)

    def test_eighth_request_is_the_last_request(self):
        self.exercise(8)

    def test_missing_budget_cannot_send_a_model_request(self):
        self.exercise(None)

if __name__ == '__main__':
    unittest.main()
