#!/usr/bin/env python3
"""Run production Swift DNS TCP framing against a local fake resolver."""
from __future__ import annotations
import socket, subprocess, tempfile, threading
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FRAMER = ROOT / "macos/NetworkShared/DNSTCPFraming.swift"
DRIVER = r'''
import Foundation
import Darwin
@main struct Driver {
  static func main() throws {
    let port = UInt16(CommandLine.arguments[1])!, mode = CommandLine.arguments[2]
    let fd = socket(AF_INET, SOCK_STREAM, 0)
    var address = sockaddr_in(); address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
    address.sin_family = sa_family_t(AF_INET); address.sin_port = port.bigEndian
    address.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))
    let connected = withUnsafePointer(to: &address) { p in p.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size)) } }
    guard connected == 0 else { exit(2) }; defer { close(fd) }
    func writeAll(_ data: Data) { data.withUnsafeBytes { raw in var offset = 0; while offset < raw.count { let n = Darwin.write(fd, raw.baseAddress!.advanced(by: offset), raw.count - offset); if n <= 0 { exit(9) }; offset += n } } }
    func readChunk() -> Data { var bytes = [UInt8](repeating: 0, count: 97); let n = Darwin.read(fd, &bytes, bytes.count); return n > 0 ? Data(bytes[0..<n]) : Data() }
    if mode == "valid" {
      var decoder = DNSTCPFraming()
      let queries = [Data(repeating: 0x11, count: 12), Data(repeating: 0x22, count: 13)]
      writeAll(try DNSTCPFraming.encode(queries[0]) + DNSTCPFraming.encode(queries[1]))
      var responses = [Data]()
      while responses.count < 2 { let chunk = readChunk(); if chunk.isEmpty { exit(3) }; try decoder.append(chunk); while let response = try decoder.next() { responses.append(response) } }
      guard responses == queries, (try decoder.next()) == nil else { exit(4) }
      print("valid PASS")
    } else if mode == "malformed-zero" || mode == "malformed-short" {
      var decoder = DNSTCPFraming(); var all = Data(); while true { let chunk = readChunk(); if chunk.isEmpty { break }; all.append(chunk) }; try decoder.append(all); do { _ = try decoder.next(); exit(5) } catch { print("\(mode) PASS") }
    } else if mode == "partial-close" {
      var decoder = DNSTCPFraming(); var bytes = Data(); while true { let chunk = readChunk(); if chunk.isEmpty { break }; bytes.append(chunk) }; try decoder.append(bytes); guard bytes.count == 5, try decoder.next() == nil else { exit(6) }; print("partial-close PASS")
    } else if mode == "oversize" {
      var decoder = DNSTCPFraming(); do { try decoder.append(Data(repeating: 0, count: 2 * 65_537 + 1)); exit(8) } catch { print("oversize PASS") }
    }
  }
}
'''

def exact(conn: socket.socket, count: int) -> bytes:
    data = bytearray()
    while len(data) < count:
        chunk = conn.recv(count - len(data))
        if not chunk: raise RuntimeError("resolver closed early")
        data.extend(chunk)
    return bytes(data)

def serve(listener: socket.socket, mode: str) -> None:
    conn, _ = listener.accept()
    with conn:
        if mode == "valid":
            frames = []
            for _ in range(2):
                header = exact(conn, 2); size = int.from_bytes(header, "big"); query = exact(conn, size); frames.append(header + query)
            for byte in b"".join(frames): conn.sendall(bytes([byte]))
        elif mode == "malformed-zero": conn.sendall(b"\x00\x00")
        elif mode == "malformed-short": conn.sendall(b"\x00\x0b" + b"x" * 11)
        elif mode == "partial-close": conn.sendall(b"\x00\x0c" + b"x" * 3)
    listener.close()

def run(binary: Path, mode: str) -> str:
    listener = socket.socket(); listener.bind(("127.0.0.1", 0)); listener.listen(1)
    thread = threading.Thread(target=serve, args=(listener, mode), daemon=True); thread.start()
    try:
        result = subprocess.run([str(binary), str(listener.getsockname()[1]), mode], capture_output=True, text=True, timeout=5)
        if result.returncode: raise RuntimeError(f"{mode}: exit {result.returncode}: {result.stderr}")
        return result.stdout.strip()
    finally: listener.close(); thread.join(timeout=2)

def main() -> None:
    with tempfile.TemporaryDirectory(prefix="myproxy-dns-framing-") as temp:
        temp = Path(temp); source = temp / "driver.swift"; binary = temp / "driver"; source.write_text(DRIVER)
        import platform
        target_arch = "arm64" if platform.machine() == "arm64" else "x86_64"
        subprocess.run(["swiftc", "-swift-version", "6", "-target", f"{target_arch}-apple-macosx14.0", str(FRAMER), str(source), "-o", str(binary)], check=True, timeout=5)
        outputs = [run(binary, mode) for mode in ("valid", "malformed-zero", "malformed-short", "partial-close", "oversize")]
        assert outputs == ["valid PASS", "malformed-zero PASS", "malformed-short PASS", "partial-close PASS", "oversize PASS"], outputs
        print("\n".join(outputs)); print("production DNS TCP framing harness: PASS")

if __name__ == "__main__": main()
