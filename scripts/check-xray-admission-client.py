#!/usr/bin/env python3
"""Exercise the compiled Xray admission client against a local mock broker."""
from __future__ import annotations

import base64
import hashlib
import hmac
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import sys

ROOT = Path(__file__).resolve().parents[1]
BUILD = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / ".planning/xray-release/swift-extension-xray"
ACTIVATION = "11111111-1111-4111-8111-111111111111"
KEY = bytes(range(32))
KEY_B64 = base64.b64encode(KEY).decode()
LEASE = "lease-lowercase-opaque-7f3a"


DRIVER = r'''
import Foundation
import MyproxyNetworkShared

@main
struct Driver {
    static func main() {
        let port = UInt16(CommandLine.arguments[1])!
        let scenario = CommandLine.arguments[2]
        let activation = "11111111-1111-4111-8111-111111111111"
        let key = Data((0..<32).map { UInt8($0) }).base64EncodedString()
        let bootstrap = try! AppAdmissionBootstrap(activation: activation, port: port, key: key)
        let data = try! JSONEncoder().encode(bootstrap)
        let client = AppAdmissionClient(data: data)!
        let request = AppAdmissionRequest(version: AppAdmissionBootstrap.version, activation: activation, nonce: "22222222-2222-4222-8222-222222222222", flowId: "33333333-3333-4333-8333-333333333333", kind: "traffic", network: "tcp", host: "example.test", hostname: "example.test", port: 443, source: AppAdmissionSource(processId: 42, userId: 501, processStart: "1:2", executablePath: "/tmp/test", bundleId: "test", signingId: "test", teamId: "TESTTEAM"))
        let started = DispatchTime.now().uptimeNanoseconds
        switch client.request(request) {
        case let .success(reply): print("OK \(scenario) \(reply.action) \(reply.lease ?? "-")")
        case let .failure(error):
            let elapsed = Double(DispatchTime.now().uptimeNanoseconds - started) / 1_000_000_000
            print("ERR \(scenario) \(String(describing: error)) \(elapsed)")
        }
    }
}
'''


def recv_exact(conn: socket.socket, size: int) -> bytes:
    data = bytearray()
    while len(data) < size:
        chunk = conn.recv(size - len(data))
        if not chunk:
            raise EOFError
        data.extend(chunk)
    return bytes(data)


def broker(scenario: str, listener: socket.socket, done: threading.Event) -> None:
    try:
        conn, _ = listener.accept()
        with conn:
            header = recv_exact(conn, 4)
            size = int.from_bytes(header, "big")
            envelope = json.loads(recv_exact(conn, size))
            payload = envelope["payload"].encode()
            request = json.loads(payload)
            if scenario == "deadline":
                time.sleep(1.35)
                return
            if scenario == "oversize":
                conn.sendall((32769).to_bytes(4, "big"))
                return
            reply = {
                "version": 1,
                "activation": request["activation"],
                "nonce": request["nonce"] if scenario != "nonce" else "44444444-4444-4444-8444-444444444444",
                "action": "proxy" if scenario == "proxy" else "direct",
                "generation": 9,
                "host": "127.0.0.1",
                "port": 9443,
                "rule": "fixture",
                "chain": ["fixture-node"],
                "relayPort": 40809 if scenario == "proxy" else None,
                "lease": LEASE if scenario == "proxy" else None,
                "password": "fixture-password" if scenario == "proxy" else None,
            }
            reply_payload = json.dumps(reply, separators=(",", ":")).encode()
            mac = hmac.new(KEY, reply_payload, hashlib.sha256).hexdigest()
            if scenario == "badmac":
                mac = "0" * 64
            response = json.dumps({"payload": reply_payload.decode(), "mac": mac}, separators=(",", ":")).encode()
            frame = len(response).to_bytes(4, "big") + response
            if scenario == "fragmented":
                for byte in frame:
                    conn.send(bytes([byte]))
                    time.sleep(0.001)
            else:
                conn.sendall(frame)
    finally:
        done.set()


def run_case(binary: Path, scenario: str) -> str:
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    done = threading.Event()
    thread = threading.Thread(target=broker, args=(scenario, listener, done), daemon=True)
    thread.start()
    try:
        result = subprocess.run([str(binary), str(listener.getsockname()[1]), scenario], capture_output=True, text=True, timeout=3)
        if result.returncode != 0:
            raise RuntimeError(f"{scenario}: driver exit {result.returncode}: {result.stderr}")
        return result.stdout.strip()
    finally:
        listener.close()
        done.wait(2)
        thread.join(2)


def main() -> None:
    if not (BUILD / "MyproxyNetworkShared.swiftmodule").exists():
        raise SystemExit(f"missing compiled shared module under {BUILD}")
    with tempfile.TemporaryDirectory(prefix="myproxy-admission-client-") as temp:
        temp_path = Path(temp)
        driver_source = temp_path / "driver.swift"
        driver_source.write_text(DRIVER)
        binary = temp_path / "driver"
        subprocess.run([
            "swiftc", "-swift-version", "6", "-D", "MYPROXY_XRAY", "-target", "arm64-apple-macosx14.0",
            "-I", str(BUILD), "-L", str(BUILD), "-lMyproxyNetworkShared",
            str(ROOT / "macos/NetworkExtension/AppAdmissionClient.swift"), str(driver_source),
            "-framework", "Network", "-framework", "NetworkExtension", "-o", str(binary),
        ], check=True)
        outputs = {case: run_case(binary, case) for case in ("direct", "proxy", "nonce", "badmac", "fragmented", "oversize", "deadline")}
        assert outputs["direct"] == "OK direct direct -", outputs["direct"]
        assert outputs["proxy"] == f"OK proxy proxy {LEASE}", outputs["proxy"]
        for case in ("nonce", "badmac", "oversize"):
            assert outputs[case].startswith(f"ERR {case} "), outputs[case]
        assert outputs["fragmented"] == "OK fragmented direct -", outputs["fragmented"]
        assert outputs["deadline"].startswith("ERR deadline timeout"), outputs["deadline"]
        elapsed = float(outputs["deadline"].split()[-1])
        assert elapsed < 1.25, outputs["deadline"]
        for case, output in outputs.items():
            print(f"{case}: {output}")
        print("admission client harness: PASS")


if __name__ == "__main__":
    main()
