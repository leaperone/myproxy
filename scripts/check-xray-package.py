#!/usr/bin/env python3
"""Verify the signed Xray channel bundle layout and isolated update feed."""
import plistlib
import sys
import subprocess
import tempfile
import zipfile
from pathlib import Path
import xml.etree.ElementTree as ET
archive_path = Path(sys.argv[1])
with zipfile.ZipFile(sys.argv[1]) as archive:
    names = archive.namelist()
    root = "MyProxy.app/Contents/"
    info = plistlib.loads(archive.read(root + "Info.plist"))
    assert info["CFBundleIdentifier"] == "local.harry.myproxy"
    assert info["CFBundleDisplayName"] == "MyProxy"
    assert info["CFBundleExecutable"] == "myproxy"
    assert info["MyproxyBuildChannel"] == "xray"
    assert info["SUFeedURL"] == "https://github.com/leaperone/myproxy/releases/download/xray/appcast.xml"
    assert all(root + "MacOS/" + name in names for name in ("myproxy", "myproxyctl", "xray"))
    assert root + "Frameworks/Sparkle.framework/" in "\n".join(names)
    extension = root + "Library/SystemExtensions/local.harry.myproxy.network-extension.systemextension/Contents/"
    assert extension + "embedded.provisionprofile" in names
    assert extension + "MacOS/MyproxyNetworkExtension" in names
    assert root + "embedded.provisionprofile" in names
    assert not any(name.endswith("/mihomo") for name in names)
    with tempfile.TemporaryDirectory() as directory:
        app = Path(directory) / "MyProxy.app"
        archive.extractall(directory)
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
        subprocess.run(["codesign", "--display", "--entitlements", "-", str(app)], check=True,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        extension_app = app / "Contents/Library/SystemExtensions/local.harry.myproxy.network-extension.systemextension"
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(extension_app)], check=True)
if len(sys.argv) == 3:
    root_element = ET.parse(sys.argv[2]).getroot()
    channel = next((element for element in root_element.iter() if element.tag.endswith("channel")), None)
    assert channel is not None
    item = next((element for element in channel if element.tag.endswith("item")), None)
    assert item is not None
    assert any(element.tag.endswith("channel") and element.text == "xray" for element in item)
    enclosure = next(element for element in item if element.tag.endswith("enclosure"))
    assert "/releases/download/xray-v" in enclosure.attrib["url"]
    assert enclosure.attrib["url"].endswith(".sparkle.zip")
    assert int(enclosure.attrib["length"]) == archive_path.stat().st_size
    assert any(key.endswith("edSignature") and value for key, value in enclosure.attrib.items())
    sparkle_version = next((element.text for element in item if element.tag.endswith("version")), None)
    short_version = next((element.text for element in item if element.tag.endswith("shortVersionString")), None)
    assert sparkle_version == info["CFBundleVersion"]
    assert short_version == info["CFBundleShortVersionString"]
print("verified signed isolated Xray bundle identity, feed, Sparkle, and Network Extension")
