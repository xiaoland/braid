#!/usr/bin/env python3
"""Capture a bounded number of OTLP/HTTP requests for the public CLI helper."""

from __future__ import annotations

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import time


class CaptureHandler(BaseHTTPRequestHandler):
    output: Path
    requests: int = 0
    last_request: float = 0.0

    def do_POST(self) -> None:  # noqa: N802 - HTTP handler contract
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        with self.output.open("ab") as stream:
            stream.write(f"PATH {self.path}\n".encode())
            stream.write(body)
            stream.write(b"\nEND\n")
        type(self).requests += 1
        type(self).last_request = time.monotonic()
        self.send_response(200)
        self.send_header("Content-Type", "application/x-protobuf")
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, _format: str, *_arguments: object) -> None:
        return


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--minimum-requests", type=int, required=True)
    parser.add_argument("--idle-seconds", type=float, default=1.0)
    parser.add_argument("--deadline-seconds", type=float, default=15.0)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    CaptureHandler.output = arguments.output
    server = ThreadingHTTPServer(("127.0.0.1", arguments.port), CaptureHandler)
    server.timeout = 0.1
    deadline = time.monotonic() + arguments.deadline_seconds
    while time.monotonic() < deadline:
        server.handle_request()
        if (
            CaptureHandler.requests >= arguments.minimum_requests
            and time.monotonic() - CaptureHandler.last_request >= arguments.idle_seconds
        ):
            return
    raise SystemExit("timed out waiting for OTLP requests")


if __name__ == "__main__":
    main()
