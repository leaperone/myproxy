#!/usr/bin/env python3
"""Exercise the production Xray Network Extension NOFILE helper in children."""
from __future__ import annotations
import platform
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "macos/NetworkShared/ProcessResourceLimits.swift"
DRIVER = r'''
import Darwin
import Foundation

@main struct Driver {
  static func main() {
    let mode = CommandLine.arguments[1]
    var limits = rlimit(rlim_cur: 256, rlim_max: mode == "low-hard" ? 512 : 16384)
    guard setrlimit(RLIMIT_NOFILE, &limits) == 0 else { exit(2) }
    guard case let .success(before) = ProcessResourceLimits.snapshot() else { exit(3) }
    guard case let .success(after) = ProcessResourceLimits.raiseXraySoftLimit() else { exit(4) }
    if mode == "low-hard" {
      guard before.soft == 256, before.hard == 512, after.soft == 512, after.hard == 512 else { exit(5) }
      print("low-hard PASS")
      return
    }
    guard before.soft == 256, after.soft == 16384, after.hard == 16384 else { exit(6) }
    var descriptors: [Int32] = []
    for _ in 0..<300 {
      let descriptor = open("/dev/null", O_RDONLY)
      guard descriptor >= 0 else { exit(7) }
      descriptors.append(descriptor)
    }
    guard descriptors.count == 300 else { exit(8) }
    descriptors.forEach { close($0) }
    print("raise-and-open PASS")
  }
}
'''

def main() -> None:
    with tempfile.TemporaryDirectory(prefix="myproxy-xray-rlimit-") as temp:
        temp = Path(temp); driver = temp / "driver.swift"; binary = temp / "driver"
        driver.write_text(DRIVER)
        arch = "arm64" if platform.machine() == "arm64" else "x86_64"
        subprocess.run(["swiftc", "-swift-version", "6", "-target", f"{arch}-apple-macosx14.0", str(SOURCE), str(driver), "-framework", "OSLog", "-o", str(binary)], check=True, timeout=10)
        for mode in ("raise", "low-hard"):
            result = subprocess.run([str(binary), mode], capture_output=True, text=True, timeout=5, check=True)
            print(result.stdout.strip())
    print("Xray resource-limit harness: PASS")

if __name__ == "__main__": main()
