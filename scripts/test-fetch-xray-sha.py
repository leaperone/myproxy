#!/usr/bin/env python3
"""Reject corrupted Xray zip downloads (local HTTP fixture, no mirrors)."""
import http.server
import io
import os
import shutil
import subprocess
import sys
import threading
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FETCH = ROOT / "scripts/fetch-xray.sh"
ASSET = "Xray-macos-arm64-v8a.zip"
VERSION = "v26.10.21"


def make_zip() -> bytes:
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("xray", b"#!/bin/sh\necho wrong\n")
    return buffer.getvalue()


class Handler(http.server.BaseHTTPRequestHandler):
    payload = b""

    def do_GET(self):
        if self.path.endswith("/" + ASSET):
            body = self.payload
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_error(404)

    def log_message(self, *_args):
        return


def main() -> int:
    if shutil.which("bash") is None:
        print("bash required", file=sys.stderr)
        return 1
    host = "127.0.0.1"
    Handler.payload = make_zip()
    server = http.server.HTTPServer((host, 0), Handler)
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://{host}:{port}/{VERSION}"
    resources = ROOT / "resources" / "xray" / "xray"
    backup = resources.read_bytes() if resources.is_file() else None
    if resources.is_file():
        resources.unlink()
    env = os.environ.copy()
    env["XRAY_VERSION"] = VERSION
    env["XRAY_RELEASE_BASE"] = base
    result = subprocess.run(
        ["bash", str(FETCH)],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    server.shutdown()
    if result.returncode == 0:
        print("expected fetch-xray.sh to fail on bad SHA-256", file=sys.stderr)
        return 1
    combined = result.stderr + result.stdout
    if "SHA-256 mismatch" not in combined:
        print("expected SHA-256 mismatch message", file=sys.stderr)
        print(combined, file=sys.stderr)
        return 1
    if resources.is_file():
        print("xray binary should not be installed after checksum failure", file=sys.stderr)
        return 1
    if backup is not None:
        resources.write_bytes(backup)
    print("fetch-xray checksum rejection fixture ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
