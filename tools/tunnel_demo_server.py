#!/usr/bin/env python3
"""
Tiny demo server for testing Datum Connect tunnels.

- GET /      -> friendly HTML page ("You've reached me over a tunnel")
- GET /big   -> large response for bandwidth testing
"""

from __future__ import annotations

import argparse
from http.server import BaseHTTPRequestHandler, HTTPServer


INDEX_HTML = """<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Datum Tunnel Demo</title>
    <style>
      body { font-family: -apple-system, system-ui, Segoe UI, Roboto, sans-serif; background: #fbfbf9; margin: 0; }
      .wrap { max-width: 760px; margin: 0 auto; padding: 56px 24px; }
      .card { background: white; border: 1px solid #eceee9; border-radius: 16px; padding: 28px 28px; box-shadow: 0 10px 28px rgba(17,24,39,0.10); }
      h1 { margin: 0 0 12px 0; font-size: 28px; color: #0f172a; }
      p { margin: 0; font-size: 16px; color: #334155; line-height: 1.5; }
      code { background: #f1f2ee; padding: 2px 6px; border-radius: 8px; }
      .small { margin-top: 16px; font-size: 13px; color: #64748b; }
    </style>
  </head>
  <body>
    <div class="wrap">
      <div class="card">
        <h1>You’ve reached me over a tunnel</h1>
        <p>
          If you can read this page, traffic is flowing through <code>datum-connect</code> and
          arriving at this local server.
        </p>
        <p class="small">
          Try <code>/big</code> to generate sustained bandwidth for the chart.
        </p>
      </div>
    </div>
  </body>
</html>
"""


class Handler(BaseHTTPRequestHandler):
    server_version = "DatumTunnelDemo/0.1"

    def do_GET(self) -> None:
        if self.path == "/":
            body = INDEX_HTML.encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        if self.path == "/big":
            size = getattr(self.server, "big_bytes", 10 * 1024 * 1024)
            body = b"x" * size
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        body = b"not found\n"
        self.send_response(404)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt: str, *args) -> None:
        # Keep output readable while still giving request visibility.
        print(f"{self.address_string()} - {fmt % args}")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=8000)
    ap.add_argument("--big-mb", type=int, default=10, help="Size of /big response in MB")
    args = ap.parse_args()

    httpd = HTTPServer((args.host, args.port), Handler)
    httpd.big_bytes = max(1, args.big_mb) * 1024 * 1024

    print(f"Listening on http://{args.host}:{args.port}")
    print("  - /     -> \"You've reached me over a tunnel\" page")
    print(f"  - /big  -> {args.big_mb}MB response")
    httpd.serve_forever()


if __name__ == "__main__":
    main()

