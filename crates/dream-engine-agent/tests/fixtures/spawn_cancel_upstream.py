"""Loopback-only deterministic OpenAI SSE fixture for Spawn cancellation."""
import json
import os
import tempfile
import re
import select
import socket
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

lock = threading.Lock()
runs = {}
events = Path(os.environ.get('SPAWN_CANCEL_EVENT_LOG', str(Path(tempfile.gettempdir()) / f'spawn-cancel-{os.getpid()}.jsonl')))

def record(run, kind, child=None):
    with lock:
        state = runs.setdefault(run, {'started': 0, 'closed': 0, 'active': 0, 'maxActive': 0, 'parents': 0})
        if kind == 'opened':
            state['started'] += 1
            state['active'] += 1
            state['maxActive'] = max(state['maxActive'], state['active'])
        elif kind == 'closed':
            state['closed'] += 1
            state['active'] -= 1
        elif kind == 'parent':
            state['parents'] += 1
        with events.open('a', encoding='utf-8') as log:
            log.write(json.dumps({'time': time.time(), 'run': run, 'event': kind, 'child': child, **state}) + '\n')

class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *_):
        pass
    def do_GET(self):
        with lock:
            body = json.dumps(runs).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def event(self, value):
        data = ('data: ' + (value if isinstance(value, str) else json.dumps(value)) + '\n\n').encode()
        self.wfile.write(f'{len(data):x}\r\n'.encode() + data + b'\r\n')
        self.wfile.flush()
    def chunk(self, delta, finish=None):
        self.event({'id': 'qa-spawn-stream', 'object': 'chat.completion.chunk', 'model': 'qa-spawn',
                    'choices': [{'index': 0, 'delta': delta, 'finish_reason': finish}]})
    def do_POST(self):
        payload = json.loads(self.rfile.read(int(self.headers.get('Content-Length', 0))))
        texts = json.dumps(payload.get('messages', []))
        child = re.search(r'QA_SPAWN_CHILD:([A-Za-z0-9_-]+):(\d)', texts)
        parent = re.search(r'QA_SPAWN_PARENT:([A-Za-z0-9_-]+)', texts)
        if not child and not parent:
            self.send_error(400, 'Expected controlled QA marker')
            return
        run = child.group(1) if child else parent.group(1)
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Cache-Control', 'no-cache')
        self.send_header('Transfer-Encoding', 'chunked')
        self.send_header('Connection', 'close')
        self.end_headers()
        self.close_connection = True
        if not child:
            record(run, 'parent')
            tasks = [{'name': f'child-{i}', 'prompt': f'QA_SPAWN_CHILD:{run}:{i}'} for i in range(5)]
            self.chunk({'role': 'assistant'})
            self.chunk({'tool_calls': [{'index': 0, 'id': 'qa-spawn-call', 'type': 'function',
                                       'function': {'name': 'Spawn', 'arguments': json.dumps({'tasks': tasks})}}]})
            self.chunk({}, 'tool_calls')
            self.event('[DONE]')
            self.wfile.write(b'0\r\n\r\n')
            self.wfile.flush()
            return
        index = int(child.group(2))
        record(run, 'opened', index)
        try:
            self.chunk({'role': 'assistant'})
            # Deliberately silent: cancellation must not wait for another token.
            while True:
                ready, _, _ = select.select([self.connection], [], [], 0.1)
                if ready and self.connection.recv(1, socket.MSG_PEEK) == b'':
                    break
        except (OSError, ConnectionError):
            pass
        finally:
            record(run, 'closed', index)

server = ThreadingHTTPServer(('127.0.0.1', 29529), Handler)
server.daemon_threads = True
print('Spawn cancellation fixture on 127.0.0.1:29529', flush=True)
server.serve_forever()
