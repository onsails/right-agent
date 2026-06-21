#!/usr/bin/env python3
# Fake Anthropic /v1/messages endpoint: captures what `claude -p` sends, returns a valid
# (streaming or non-streaming) Anthropic response so claude completes a turn.
# No secrets, no real model, no Anthropic quota — claude is redirected here via ANTHROPIC_BASE_URL.
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
CAP = sys.argv[2] if len(sys.argv) > 2 else "/tmp/cap.jsonl"

def sse(events):
    out = []
    for ev, data in events:
        out.append(f"event: {ev}\ndata: {json.dumps(data)}\n\n")
    return "".join(out).encode()

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _read(self):
        n = int(self.headers.get("content-length", 0))
        return self.rfile.read(n) if n else b""
    def do_POST(self):
        body = self._read()
        try: parsed = json.loads(body)
        except: parsed = {"_raw": body[:500].decode("utf-8", "replace")}
        # capture (redact auth header value -> scheme + length only)
        auth = self.headers.get("authorization") or self.headers.get("x-api-key") or ""
        hdr_summary = {
            "auth_header": ("authorization" if self.headers.get("authorization") else ("x-api-key" if self.headers.get("x-api-key") else None)),
            "auth_scheme": (auth.split(" ")[0] if auth.startswith("Bearer ") else ("x-api-key" if self.headers.get("x-api-key") else "raw")),
            "auth_len": len(auth),
            "anthropic_version": self.headers.get("anthropic-version"),
            "anthropic_beta": self.headers.get("anthropic-beta"),
            "user_agent": self.headers.get("user-agent"),
            "x_app": self.headers.get("x-app"),
        }
        rec = {"path": self.path, "headers": hdr_summary, "body": parsed}
        with open(CAP, "a") as f: f.write(json.dumps(rec) + "\n")

        # count_tokens endpoint
        if "count_tokens" in self.path:
            payload = json.dumps({"input_tokens": 12}).encode()
            self.send_response(200); self.send_header("content-type","application/json")
            self.send_header("content-length", str(len(payload))); self.end_headers()
            self.wfile.write(payload); return

        streaming = bool(parsed.get("stream"))
        # If claude forced a tool (structured output), echo a tool_use; else plain text.
        tools = parsed.get("tools") or []
        forced = parsed.get("tool_choice")
        text = "PONG"
        if streaming:
            self.send_response(200)
            self.send_header("content-type","text/event-stream"); self.end_headers()
            ev = [
              ("message_start", {"type":"message_start","message":{"id":"msg_test","type":"message","role":"assistant","model":parsed.get("model","test"),"content":[],"stop_reason":None,"stop_sequence":None,"usage":{"input_tokens":12,"output_tokens":1}}}),
              ("content_block_start", {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
              ("content_block_delta", {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
              ("content_block_stop", {"type":"content_block_stop","index":0}),
              ("message_delta", {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":None},"usage":{"output_tokens":1}}),
              ("message_stop", {"type":"message_stop"}),
            ]
            self.wfile.write(sse(ev))
        else:
            payload = json.dumps({"id":"msg_test","type":"message","role":"assistant","model":parsed.get("model","test"),
              "content":[{"type":"text","text":text}],"stop_reason":"end_turn","stop_sequence":None,
              "usage":{"input_tokens":12,"output_tokens":1}}).encode()
            self.send_response(200); self.send_header("content-type","application/json")
            self.send_header("content-length", str(len(payload))); self.end_headers()
            self.wfile.write(payload)

HTTPServer(("127.0.0.1", PORT), H).serve_forever()
