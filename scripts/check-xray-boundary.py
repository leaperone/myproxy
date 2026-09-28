#!/usr/bin/env python3
"""Preserve original release behavior while allowing synchronized package bumps."""
import os
import plistlib
import re
import subprocess
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def verify_version_metadata(original_lock, current_lock, original_plists, current_plists, version):
    assert re.fullmatch(r"\d+\.\d+\.\d+", version), "invalid package version"
    locks = []
    for text, expected in [(original_lock, None), (current_lock, version)]:
        lock = tomllib.loads(text)
        packages = [package for package in lock["package"] if package["name"] == "myproxy" and "source" not in package]
        assert len(packages) == 1, "expected exactly one local myproxy package"
        if expected is not None:
            assert packages[0]["version"] == expected, "Cargo.lock version does not match Cargo.toml"
        packages[0]["version"] = "PACKAGE_VERSION"
        locks.append(lock)
    assert locks[0] == locks[1], "protected Cargo.lock dependency data changed"
    assert original_plists.keys() == current_plists.keys(), "packaging metadata files differ"
    for name, previous in original_plists.items():
        previous = plistlib.loads(previous)
        current = plistlib.loads(current_plists[name])
        for key in ("CFBundleShortVersionString", "CFBundleVersion"):
            assert current[key] == version, f"{name}: {key} does not match Cargo.toml"
            previous.pop(key)
            current.pop(key)
        assert previous == current, f"protected packaging metadata changed: {name}"


def main():
    base = os.environ.get("MYPROXY_BOUNDARY_BASE", "67dd4c196d4f0e3408e3efdd426e6baafbf79102")
    protected = (
        ".github/workflows/release.yml", ".github/workflows/ci.yml",
        "scripts/release-macos.sh", "scripts/package-macos-app.sh",
        "scripts/fetch-mihomo.sh", "scripts/check-release-artifacts.py",
        "scripts/sparkle_previous_tags.py",
    )
    changed = subprocess.check_output(["git", "diff", "--name-only", base, "--", *protected], cwd=ROOT, text=True)
    assert not changed, "protected files changed: " + changed
    original = lambda name: subprocess.check_output(["git", "show", base + ":" + name], cwd=ROOT)
    plists = ("packaging/macos/Info.plist", "packaging/macos/NetworkExtension/Info.plist")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    verify_version_metadata(original("Cargo.lock").decode(), (ROOT / "Cargo.lock").read_text(),
        {name: original(name) for name in plists}, {name: (ROOT / name).read_bytes() for name in plists}, version)
    backend = (ROOT / "src/backend.rs").read_text()
    assert 'cfg!(feature = "xray-channel")' in backend
    assert "fs::" not in backend, "backend cannot be changed by sharing a runtime config"
    updates = (ROOT / "src/updates.rs").read_text()
    for feed in ("latest/download/appcast.xml", "download/nightly/appcast.xml", "download/xray/appcast.xml"):
        assert "https://github.com/leaperone/myproxy/releases/" + feed in updates
    compiler = (ROOT / "src/compile.rs").read_text()
    compiler = re.sub(r"(?m)^[ \t]*unavailable_fallback: Default::default\(\),\n", "", compiler)
    assert compiler == original("src/compile.rs").decode(), "default compiler behavior changed"
    print("original build/release paths preserved; package versions synchronized; Xray requires a separate build feature")


if __name__ == "__main__":
    main()
