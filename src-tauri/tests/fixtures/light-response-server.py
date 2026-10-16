#!/usr/bin/env python3
"""Loopback-only synthetic model for real Tauri UI acceptance, never cloud evidence.
No prompt bodies or credentials are logged. Stop with Ctrl-C after the UI check.
"""
import argparse
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

MODEL = 'sophonote-ui-fixture'
lock = threading.Lock()
audit = []

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def reply(self, status, data):
        encoded = json.dumps(data, ensure_ascii=False).encode()
        try:
            self.send_response(status)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self):
        if self.path == '/v1/models':
            self.reply(200, {'object': 'list', 'data': [{'id': MODEL, 'object': 'model', 'owned_by': 'local-acceptance'}]})
        else:
            self.reply(404, {'error': 'fixture endpoint not found'})

    def do_POST(self):
        if self.path != '/v1/chat/completions':
            self.reply(404, {'error': 'fixture endpoint not found'})
            return
        try:
            size = int(self.headers.get('Content-Length', '0'))
            if size < 1 or size > 2 * 1024 * 1024:
                raise ValueError('size')
            request = json.loads(self.rfile.read(size))
            messages = request['messages']
            if request.get('model') != MODEL or not isinstance(messages, list):
                raise ValueError('request')
            users = [m for m in messages if m.get('role') == 'user']
            last = users[-1]['content'] if users else ''
            if not isinstance(last, str):
                raise ValueError('content')
        except (ValueError, KeyError, TypeError):
            self.reply(400, {'error': 'invalid fixture request'})
            return
        scenario = 'cancel' if 'STOP_LOCAL_815' in last else 'response'
        with lock:
            audit.append({'scenario': scenario, 'userTurns': len(users), 'assistantTurns': sum(m.get('role') == 'assistant' for m in messages), 'tools': len(request.get('tools', [])), 'stream': bool(request.get('stream')), 'timestamp': int(time.time())})
            self.server.audit_path.write_text(json.dumps({'synthetic': True, 'requests': audit}, indent=2))
        if scenario == 'cancel':
            time.sleep(55)
        content = f'本机验收响应 LOCAL_READY_812。已收到 {len(users)} 轮用户输入；这不是云端模型结果。'
        self.reply(200, {'id': 'local-fixture', 'object': 'chat.completion', 'model': MODEL,
            'choices': [{'index': 0, 'message': {'role': 'assistant', 'content': content}, 'finish_reason': 'stop'}]})

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--port', type=int, default=0)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    server = ThreadingHTTPServer(('127.0.0.1', args.port), Handler)
    server.daemon_threads = True
    server.audit_path = args.output / 'local-model-requests.json'
    if server.audit_path.exists():
        audit.extend(json.loads(server.audit_path.read_text()).get('requests', []))
    endpoint = f'http://127.0.0.1:{server.server_port}/v1'
    (args.output / 'local-model-endpoint.json').write_text(json.dumps({'endpoint': endpoint, 'model': MODEL, 'synthetic': True}))
    print(endpoint, flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
