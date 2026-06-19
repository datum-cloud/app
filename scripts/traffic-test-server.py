#!/usr/bin/env python3
"""Minimal HTTP server for exercising Datum tunnel traffic metrics."""

from http.server import BaseHTTPRequestHandler, HTTPServer

HOST = "127.0.0.1"
PORT = 3001
PAYLOAD_KB = 64


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path in ("/", "/index.html"):
            body = f"""<!DOCTYPE html>
<html>
  <head><title>Datum traffic test</title></head>
  <body style="font-family: system-ui, sans-serif; padding: 2rem;">
    <h1>Traffic test server</h1>
    <p>Listening on <code>http://{HOST}:{PORT}</code></p>
    <ul>
      <li><a href="/ping">/ping</a> — small response</li>
      <li><a href="/data">/data</a> — {PAYLOAD_KB} KB payload</li>
    </ul>
    <button id="burst">Generate burst (10× /data)</button>
    <pre id="log"></pre>
    <script>
      const log = document.getElementById("log");
      document.getElementById("burst").onclick = async () => {{
        log.textContent = "Fetching...";
        await Promise.all(Array.from({{length: 10}}, () => fetch("/data")));
        log.textContent = "Done — sent 10 requests";
      }};
    </script>
  </body>
</html>
""".encode()
            self._respond(200, "text/html; charset=utf-8", body)
            return

        if self.path == "/ping":
            self._respond(200, "text/plain; charset=utf-8", b"ok\n")
            return

        if self.path == "/data":
            body = b"x" * (PAYLOAD_KB * 1024)
            self._respond(200, "application/octet-stream", body)
            return

        self._respond(404, "text/plain; charset=utf-8", b"not found\n")

    def _respond(self, status: int, content_type: str, body: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args) -> None:
        print(f"[{self.log_date_time_string()}] {format % args}")


def main() -> None:
    server = HTTPServer((HOST, PORT), Handler)
    print(f"Traffic test server running at http://{HOST}:{PORT}")
    print("  /ping  — small response")
    print(f"  /data  — {PAYLOAD_KB} KB download")
    server.serve_forever()


if __name__ == "__main__":
    main()
