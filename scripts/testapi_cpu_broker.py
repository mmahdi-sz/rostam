"""CI-only CPU broker fixture; media processing still runs in the real handlers."""
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class BrokerHandler(BaseHTTPRequestHandler):
    def reply(self, status, payload):
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path.split("?")[0] == "/cpu/user_cores":
            self.reply(200, {"is_busy": False})
        else:
            self.reply(404, {})

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if self.path == "/cpu/acquire":
            self.reply(200, {"cores": [min(os.sched_getaffinity(0))]})
        elif self.path == "/cpu/release":
            self.reply(200, {"ok": True})
        else:
            self.reply(404, {})


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", 6589), BrokerHandler).serve_forever()
