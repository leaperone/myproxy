#!/usr/bin/env python3
"""Validate the signed Sparkle feed and bundle identity before publishing."""

import argparse
from pathlib import Path
import plistlib
import xml.etree.ElementTree as ET
import zipfile


def require(condition, message):
    if not condition:
        raise SystemExit(message)


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("channel", choices=("prod", "nightly"))
parser.add_argument("version")
parser.add_argument("build_number")
parser.add_argument("tag")
parser.add_argument("--dist", type=Path, default=Path("dist"))
args = parser.parse_args()
require(args.tag == f"v{args.version}", "release tag must match the display version")

sparkle = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"
archive_name = f"myproxy-{args.version}.sparkle.zip"
archive = args.dist / archive_name
release_url = f"https://github.com/leaperone/myproxy/releases/download/{args.tag}/"
feed_url = "https://github.com/leaperone/myproxy/releases/" + (
    "download/nightly/appcast.xml" if args.channel == "nightly" else "latest/download/appcast.xml"
)

items = ET.parse(args.dist / "appcast.xml").findall("./channel/item")
require(len(items) == 1, "appcast must contain only the current release")
item = items[0]
enclosure = item.find("enclosure")
require(enclosure is not None, "missing full update enclosure")
require(enclosure.get("url") == release_url + archive_name, "wrong archive download URL")
require(int(enclosure.get("length", "0")) == archive.stat().st_size, "wrong archive size")
require(enclosure.get(sparkle + "edSignature"), "missing Sparkle EdDSA signature")
require(
    (item.findtext(sparkle + "version") or enclosure.get(sparkle + "version")) == args.build_number,
    "appcast build number does not match the release",
)
require(
    (item.findtext(sparkle + "shortVersionString") or enclosure.get(sparkle + "shortVersionString")) == args.version,
    "appcast display version does not match the release",
)
require(
    item.findtext(sparkle + "channel", "") == ("nightly" if args.channel == "nightly" else ""),
    "appcast contains an update from the wrong channel",
)
for delta in item.findall(f"{sparkle}deltas/enclosure"):
    require(delta.get("url", "").startswith(release_url), "delta uses the wrong release URL")
    require(delta.get(sparkle + "edSignature"), "unsigned delta")

with zipfile.ZipFile(archive) as bundle:
    info = plistlib.loads(bundle.read("myproxy.app/Contents/Info.plist"))
    require(info["CFBundleVersion"] == args.build_number, "bundle build number mismatch")
    require(info["CFBundleShortVersionString"] == args.version, "bundle version mismatch")
    require(info["MyproxyBuildChannel"] == args.channel, "bundle channel mismatch")
    require(info["SUFeedURL"] == feed_url, "bundle update feed mismatch")

print(f"validated {args.channel} {args.version} (build {args.build_number})")
