"""Serves web/ for local use: python3 web/serve.py [PORT] (default 8770).

Adds the cross-origin isolation headers (COOP same-origin, COEP require-corp) that SharedArrayBuffer,
and so the threaded WebAssembly build, needs. Any static host that can set these two headers works
the same way; without them the page falls back to the single-threaded build.
"""
import functools
import http.server
import sys
from pathlib import Path


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cache-Control", "no-cache")
        super().end_headers()


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8770
    handler = functools.partial(Handler, directory=str(Path(__file__).parent))
    print(f"http://localhost:{port}/")
    http.server.ThreadingHTTPServer(("", port), handler).serve_forever()
