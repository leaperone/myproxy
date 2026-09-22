#!/usr/bin/env python3
"""Publish only a completed, verified Xray build; never tag a failed build."""
import hashlib
import json
import os
import plistlib
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

from xray_release_metadata import validate_release

SPARKLE = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"


def gh(*args):
    return subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout


def item_version(item):
    enclosure = item.find("enclosure")
    return item.findtext(SPARKLE + "version") or enclosure.get(SPARKLE + "version")


def verify_artifacts(directory, version, build, tag, archive_name, repository):
    validate_release(version, build, tag, archive_name)
    archive = directory / archive_name
    appcast = directory / "appcast.xml"
    checksum = directory / (archive_name + ".sha256")
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    recorded, recorded_name = checksum.read_text().split()
    if digest != recorded or Path(recorded_name).name != archive_name:
        raise ValueError("Xray archive checksum does not match")
    with zipfile.ZipFile(archive) as bundle:
        info = plistlib.loads(bundle.read("MyProxy.app/Contents/Info.plist"))
    if (info["CFBundleShortVersionString"], info["CFBundleVersion"], info["MyproxyBuildChannel"]) != (version, build, "xray"):
        raise ValueError("Xray bundle does not match the release metadata")
    item = ET.parse(appcast).getroot().find("channel/item")
    enclosure = item.find("enclosure")
    if item.findtext(SPARKLE + "channel") != "xray" or item_version(item) != build:
        raise ValueError("Xray feed channel or build does not match")
    short_version = item.findtext(SPARKLE + "shortVersionString") or enclosure.get(SPARKLE + "shortVersionString")
    if short_version != version:
        raise ValueError("Xray feed display version does not match the bundle")
    if enclosure.get("url") != f"https://github.com/{repository}/releases/download/{tag}/{archive_name}":
        raise ValueError("Xray feed points to a different release")
    if int(enclosure.get("length")) != archive.stat().st_size or not enclosure.get(SPARKLE + "edSignature"):
        raise ValueError("Xray feed size or signature is missing")
    return [archive, appcast, checksum]


def main():
    directory = Path(sys.argv[1]).resolve()
    repository = os.environ["GITHUB_REPOSITORY"]
    source = os.environ["GITHUB_SHA"]
    version, build, tag, archive_name = (os.environ[key] for key in (
        "XRAY_VERSION", "XRAY_BUILD_NUMBER", "XRAY_TAG", "XRAY_ARCHIVE"
    ))
    if os.environ["GITHUB_REF"] != "refs/heads/xray":
        raise ValueError("Only the xray branch publishes the Xray channel")
    if subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip() != source:
        raise ValueError("The checked-out source is not the verified build")
    assets = verify_artifacts(directory, version, build, tag, archive_name, repository)
    releases = json.loads(gh("api", f"repos/{repository}/releases?per_page=100", "--paginate", "--slurp"))
    by_tag = {release["tag_name"]: release for page in releases for release in page}
    references = json.loads(gh("api", f"repos/{repository}/git/matching-refs/tags/{tag}"))
    for reference in references:
        if reference["ref"] != f"refs/tags/{tag}":
            continue
        if reference["object"]["type"] != "commit" or reference["object"]["sha"] != source:
            raise ValueError("Existing Xray version tag points to different source")
    with tempfile.TemporaryDirectory() as temporary:
        temporary = Path(temporary)
        pointer = by_tag.get("xray")
        if pointer:
            gh("release", "download", "xray", "--pattern", "appcast.xml", "--dir", str(temporary))
            previous = item_version(ET.parse(temporary / "appcast.xml").getroot().find("channel/item"))
            if tuple(map(int, previous.split("."))) > tuple(map(int, build.split("."))):
                raise ValueError("A newer Xray build is already published")
        notes = temporary / "notes.md"
        notes.write_text(f"myproxy Xray {version}\n\n" + Path("packaging/macos/Xray-ReleaseNotes.md").read_text()
                         + f"\n\nSource: `{source}`\n")
        existing = by_tag.get(tag)
        if existing:
            reference = json.loads(gh("api", f"repos/{repository}/git/ref/tags/{tag}"))["object"]
            if reference["type"] != "commit" or reference["sha"] != source:
                raise ValueError("Existing Xray version tag points to different source")
            remote_assets = {asset["name"]: asset.get("digest") for asset in existing["assets"]}
            if remote_assets != {asset.name: "sha256:" + hashlib.sha256(asset.read_bytes()).hexdigest() for asset in assets}:
                raise ValueError("Existing Xray release assets differ; do not overwrite them")
        else:
            gh("release", "create", tag, *(str(asset) for asset in assets), "--draft", "--prerelease",
               "--latest=false", "--target", source, "--title", f"myproxy Xray {version}", "--notes-file", str(notes))
        gh("release", "edit", tag, "--draft=false", "--prerelease", "--latest=false")
        pointer_notes = temporary / "pointer.md"
        pointer_notes.write_text(f"Latest Xray: https://github.com/{repository}/releases/tag/{tag}\n")
        if pointer:
            gh("release", "upload", "xray", str(assets[1]), "--clobber")
            gh("api", "--method", "PATCH", f"repos/{repository}/git/refs/tags/xray", "-f", f"sha={source}", "-F", "force=true")
            gh("release", "edit", "xray", "--prerelease", "--latest=false", "--title", "myproxy Xray update channel", "--notes-file", str(pointer_notes))
        else:
            gh("release", "create", "xray", str(assets[1]), "--target", source, "--prerelease", "--latest=false",
               "--title", "myproxy Xray update channel", "--notes-file", str(pointer_notes))
    print(f"Published https://github.com/{repository}/releases/tag/{tag}")


if __name__ == "__main__":
    main()
