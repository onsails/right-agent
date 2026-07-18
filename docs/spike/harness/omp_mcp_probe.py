#!/usr/bin/env python3
"""Minimal MCP streamable-http server for probing omp's MCP client.
Logs auth headers + all JSON-RPC methods to /tmp/omp-probe/mcp-log.txt.
Serves one tool: probe_ping.
"""
import json
from http.server import BaseHTTPRequestHandler, HTTPServer

LOG = "/tmp/omp-probe/mcp-log.txt"
PORT = 18100

TOOLS = [{
    "name": "probe_ping",
    "description": "Ping the probe server. Call this when the user asks to ping the probe.",
    "inputSchema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]},
}]


def log(line):
    with open(LOG, "a") as f:
        f.write(line + "\n")


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _send(self, code, obj=None, session=None):
        body = b"" if obj is None else json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        if session:
            self.send_header("Mcp-Session-Id", session)
        self.end_headers()
        if body:
            self.wfile.write(body)

    def do_GET(self):
        log(f"GET auth={self.headers.get('Authorization')!r} accept={self.headers.get('Accept')!r}")
        self._send(405, {"error": "no standalone GET stream"})

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(n)
        auth = self.headers.get("Authorization")
        try:
            msg = json.loads(raw)
        except Exception:
            log(f"POST unparseable auth={auth!r} body={raw[:200]!r}")
            self._send(400, {"error": "bad json"})
            return
        method = msg.get("method", "?")
        mid = msg.get("id")
        log(f"POST method={method} id={mid} auth={auth!r}")

        if method == "initialize":
            self._send(200, {
                "jsonrpc": "2.0", "id": mid,
                "result": {
                    "protocolVersion": msg.get("params", {}).get("protocolVersion", "2025-03-26"),
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "probe", "version": "0.0.1"},
                },
            }, session="probe-session-1")
        elif method == "notifications/initialized":
            self._send(202)
        elif method == "tools/list":
            self._send(200, {"jsonrpc": "2.0", "id": mid, "result": {"tools": TOOLS}})
        elif method == "tools/call":
            name = msg.get("params", {}).get("name")
            args = msg.get("params", {}).get("arguments", {})
            log(f"TOOL-CALL name={name} args={args}")
            self._send(200, {"jsonrpc": "2.0", "id": mid, "result": {
                "content": [{"type": "text", "text": f"pong:{args.get('msg', '')}"}],
            }})
        else:
            self._send(200, {"jsonrpc": "2.0", "id": mid, "result": {}})

    def log_message(self, *a):
        pass


if __name__ == "__main__":
    log("=== server start ===")
    HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
