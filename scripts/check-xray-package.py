#!/usr/bin/env python3
"""Verify the signed Xray channel bundle layout and isolated update feed."""
import plistlib
import sys
import subprocess
import tempfile
import zipfile
from pathlib import Path
import xml.etree.ElementTree as ET
import datetime
import fnmatch
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
    assert archive.read(root + "Resources/ThirdParty/xray/LICENSE").startswith(b"Mozilla Public License Version 2.0")
    assert b"v26.9.9" in archive.read(root + "Resources/ThirdParty/xray/NOTICE.md")
    assert not any(name.endswith("/mihomo") for name in names)
    with tempfile.TemporaryDirectory() as directory:
        app = Path(directory) / "MyProxy.app"
        subprocess.run(["ditto", "-x", "-k", str(archive_path), directory], check=True)
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
        extension_app = app / "Contents/Library/SystemExtensions/local.harry.myproxy.network-extension.systemextension"
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(extension_app)], check=True)
        for signed_bundle, identifier in ((app, "local.harry.myproxy"), (extension_app, "local.harry.myproxy.network-extension")):
            details = subprocess.run(["codesign", "-d", "--verbose=4", str(signed_bundle)], check=True, capture_output=True, text=True).stderr
            assert "Authority=Developer ID Application:" in details and "(runtime)" in details
            assert "TeamIdentifier=5UAHRS482C" in details and "Identifier=" + identifier in details
            entitlements = plistlib.loads(subprocess.run(["codesign", "--display", "--entitlements", "-", "--xml", str(signed_bundle)], check=True, capture_output=True).stdout)
            profile = plistlib.loads(subprocess.run(["security", "cms", "-D", "-i", str(signed_bundle / "Contents/embedded.provisionprofile")], check=True, capture_output=True).stdout)
            expiry = profile["ExpirationDate"].replace(tzinfo=datetime.timezone.utc)
            assert expiry > datetime.datetime.now(datetime.timezone.utc), "expired provisioning profile"
            grants = profile["Entitlements"]
            app_id = "5UAHRS482C." + identifier
            assert entitlements["com.apple.application-identifier"] == app_id
            allowed_id = grants.get("com.apple.application-identifier", grants.get("application-identifier", ""))
            assert fnmatch.fnmatchcase(app_id, allowed_id), "profile does not authorize bundle"
            assert not entitlements.get("com.apple.security.get-task-allow", False)
            key = "com.apple.developer.networking.networkextension"
            assert entitlements.get(key), "missing Network Extension entitlement"
            assert all(any(fnmatch.fnmatchcase(value, grant) for grant in grants.get(key, [])) for value in entitlements[key]), "profile does not authorize Network Extension capabilities"
            # macOS Team-ID app groups do not require developer-portal registration:
            # https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.application-groups
            assert entitlements.get("com.apple.security.application-groups") == ["5UAHRS482C.local.harry.myproxy"], "wrong shared app group"
        core_details = subprocess.run(["codesign", "-d", "--verbose=4", str(app / "Contents/MacOS/xray")], check=True, capture_output=True, text=True).stderr
        assert "Identifier=local.harry.myproxy.xray" in core_details
        assert "TeamIdentifier=5UAHRS482C" in core_details
        subprocess.run(["xcrun", "stapler", "validate", str(app)], check=True, capture_output=True)
        subprocess.run(["spctl", "--assess", "--type", "execute", str(app)], check=True, capture_output=True)
if len(sys.argv) == 3:
    root_element = ET.parse(sys.argv[2]).getroot()
    channel = next((element for element in root_element.iter() if element.tag.endswith("channel")), None)
    assert channel is not None
    item = next((element for element in channel if element.tag.endswith("item")), None)
    assert item is not None
    assert any(element.tag.endswith("channel") and element.text == "xray" for element in item)
    enclosure = next(element for element in item if element.tag.endswith("enclosure"))
    assert "/releases/download/xray-v" in enclosure.attrib["url"]
    assert enclosure.attrib["url"] == "https://github.com/leaperone/myproxy/releases/download/xray-v" + info["CFBundleShortVersionString"] + "/" + archive_path.name
    assert int(enclosure.attrib["length"]) == archive_path.stat().st_size
    assert any(key.endswith("edSignature") and value for key, value in enclosure.attrib.items())
    sparkle_version = item.findtext("{http://www.andymatuschak.org/xml-namespaces/sparkle}version") or enclosure.get("{http://www.andymatuschak.org/xml-namespaces/sparkle}version")
    short_version = item.findtext("{http://www.andymatuschak.org/xml-namespaces/sparkle}shortVersionString") or enclosure.get("{http://www.andymatuschak.org/xml-namespaces/sparkle}shortVersionString")
    assert sparkle_version == info["CFBundleVersion"]
    assert short_version == info["CFBundleShortVersionString"]
print("verified signed isolated Xray bundle identity, feed, Sparkle, and Network Extension")
