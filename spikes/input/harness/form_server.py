#!/usr/bin/python3
"""Spike 3 browser target: serves a form with one autofocused field; the submitted value goes to argv[2]."""
import sys
import urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

PAGE = b"""<!doctype html><meta charset="utf-8"><title>handOs spike</title>
<form action="/submit"><input name="t" autofocus style="font-size:24px;width:90%"></form>"""


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        url = urllib.parse.urlparse(self.path)
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        if url.path == "/submit":
            value = urllib.parse.parse_qs(url.query, keep_blank_values=True).get("t", [""])[0]
            with open(sys.argv[2], "w", encoding="utf-8") as f:
                f.write(value)
            self.wfile.write(b"ok")
        else:
            self.wfile.write(PAGE)

    def log_message(self, *args):
        pass


HTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
