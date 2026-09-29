#!/usr/bin/env python3
"""Check build contents without launching an app or exercising user traffic."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import struct
import zipfile


def android(package):
    required = ["assets/index.android.bundle", "lib/arm64-v8a/libgojni.so", "lib/arm64-v8a/libmyproxy_mobile.so"]
    with zipfile.ZipFile(package) as archive:
        for name in required:
            info = archive.getinfo(name)
            if info.file_size == 0:
                raise ValueError(f"Empty packaged component: {name}")
        for name in required[1:]:
            with archive.open(name) as binary:
                header = binary.read(64)
                if header[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", header, 18)[0] != 183:
                    raise ValueError(f"Native library is not ARM64 ELF: {name}")
                offset = struct.unpack_from("<Q", header, 32)[0]
                stride, count = struct.unpack_from("<HH", header, 54)
                for index in range(count):
                    binary.seek(offset + index * stride)
                    segment = binary.read(stride)
                    if struct.unpack_from("<I", segment)[0] == 1 and struct.unpack_from("<Q", segment, 48)[0] < 16384:
                        raise ValueError(f"Native library does not support 16 KiB pages: {name}")
    return {"platform": "android", "components": required, "sha256": hashlib.file_digest(package.open("rb"), "sha256").hexdigest()}


def ios(application):
    extension = application / "PlugIns/MyProxyPacketTunnel.appex"
    with (application / "Info.plist").open("rb") as stream:
        host = plistlib.load(stream)
    with (extension / "Info.plist").open("rb") as stream:
        provider = plistlib.load(stream)
    if host["CFBundleIdentifier"] != "one.leaper.myproxy.xray" or provider["CFBundleIdentifier"] != "one.leaper.myproxy.xray.PacketTunnel":
        raise ValueError("Unexpected mobile application identity")
    for key in ("CFBundleVersion", "CFBundleShortVersionString"):
        if host[key] != provider[key]:
            raise ValueError(f"App and Packet Tunnel {key} differ")
    for directory, info in ((application, host), (extension, provider)):
        if not (directory / info["CFBundleExecutable"]).is_file():
            raise ValueError("A native executable is missing")
    if not (application / "main.jsbundle").is_file():
        raise ValueError("Offline JavaScript bundle is missing")
    return {"platform": "ios", "version": host["CFBundleShortVersionString"], "build": host["CFBundleVersion"], "extension": provider["CFBundleIdentifier"], "signing": "unsigned build"}


parser = argparse.ArgumentParser()
parser.add_argument("platform", choices=("ios", "android"))
parser.add_argument("package", type=Path)
parser.add_argument("--report", type=Path, required=True)
args = parser.parse_args()
report = android(args.package) if args.platform == "android" else ios(args.package)
report["source"] = os.environ.get("GITHUB_SHA")
report["device_tested"] = False
args.report.parent.mkdir(parents=True, exist_ok=True)
args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
print(json.dumps(report, ensure_ascii=False))
