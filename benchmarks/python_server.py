"""Standard-library baseline with persistent HTTP/1.1 and access logging off."""

import functools
import http.server
import sys


class Handler(http.server.SimpleHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass


if __name__ == "__main__":
    http.server.ThreadingHTTPServer(
        ("127.0.0.1", int(sys.argv[2])),
        functools.partial(Handler, directory=sys.argv[1]),
    ).serve_forever()
