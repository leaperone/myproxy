#!/usr/bin/env python3
"""Verify that a test app cannot update or replace the production bundle."""
import plistlib
import sys
import zipfile
from pathlib import Path
with zipfile.ZipFile(sys.argv[1]) as archive:
    names = archive.namelist()
    root = "MyProxy Xray.app/Contents/"
    info = plistlib.loads(archive.read(root + "Info.plist"))
    assert info["CFBundleIdentifier"] == "one.leaper.myproxy.xray-test"
    assert info["CFBundleExecutable"] == "myproxy"
    assert not any(key.startswith("SU") for key in info), "production update feed in test app"
    assert all(root + "MacOS/" + name in names for name in ("myproxy", "myproxyctl", "xray"))
    assert not any(".systemextension/" in name or "Sparkle.framework/" in name for name in names)
    assert not any(name.endswith("/mihomo") for name in names)
print("verified isolated Xray bundle identity and contents")
