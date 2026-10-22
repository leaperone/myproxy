#!/usr/bin/env python3
"""Preserve original release behavior while allowing synchronized package bumps."""
import os
import plistlib
import re
import subprocess
import tomllib
from copy import deepcopy
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

REGISTRY_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"

APPROVED_CA_PACKAGES = (
    {
        "name": "rustls-webpki",
        "old_version": "0.103.15",
        "new_version": "0.103.16",
        "old_checksum": "f3c3cf1d8b1e7d4927e2d154c3fcb02979afb9939629c62cd9048d4f07b60ac2",
        "new_checksum": "4443b1b6d39d7c663139942cc4558262562d2c84123bb6282ef177009ca8b281",
    },
    {
        "name": "webpki-roots",
        "old_version": "1.0.9",
        "new_version": "1.0.10",
        "old_checksum": "7dcd9d09a39985f5344844e66b0c530a33843579125f23e21e9f0f220850f22a",
        "new_checksum": "1c1c8e20c8a38fc7d65abfe74eb7280e4aa667e8eec8bc84996d2874810748d9",
    },
    {
        "name": "webpki-root-certs",
        "old_version": "1.0.9",
        "new_version": "1.0.10",
        "old_checksum": "b96554aa2acc8ccdb7e1c9a58a7a68dd5d13bccc69cd124cb09406db612a1c9b",
        "new_checksum": "58ec80e73fd8baf57bf0d9615554c75348bda990d6d5bbeab964dd068319fecb",
    },
)

WEBPKI_ROOTS_NEW_DEP = "webpki-roots 1.0.10"
WEBPKI_ROOTS_OLD_DEP = "webpki-roots 1.0.9"


def _baseline_packages(lock):
    return {
        (package["name"], package["version"]): package
        for package in lock["package"]
        if "source" in package
    }


def normalize_approved_ca_updates(current_lock, baseline_lock):
    """Rewrite reviewed CA bumps on current_lock so it can be compared to baseline_lock."""
    current = deepcopy(current_lock)
    baseline = _baseline_packages(baseline_lock)
    webpki_roots_bumped = False

    for package in current["package"]:
        if "source" not in package:
            continue
        for rule in APPROVED_CA_PACKAGES:
            if package["name"] != rule["name"]:
                continue
            if package["version"] != rule["new_version"]:
                continue
            baseline_entry = baseline.get((rule["name"], rule["old_version"]))
            assert baseline_entry is not None, f"missing baseline entry for {rule['name']}"
            assert package["source"] == REGISTRY_SOURCE, f"{rule['name']}: source must stay official registry"
            assert package["checksum"] == rule["new_checksum"], f"{rule['name']}: checksum does not match approved CA update"
            assert package.get("dependencies", []) == baseline_entry.get("dependencies", []), (
                f"{rule['name']}: dependency graph changed beyond approved CA update"
            )
            package["version"] = rule["old_version"]
            package["checksum"] = rule["old_checksum"]
            if rule["name"] == "webpki-roots":
                webpki_roots_bumped = True

    if webpki_roots_bumped:
        for package in current["package"]:
            dependencies = package.get("dependencies")
            if not dependencies:
                continue
            package["dependencies"] = [
                WEBPKI_ROOTS_OLD_DEP if dependency == WEBPKI_ROOTS_NEW_DEP else dependency
                for dependency in dependencies
            ]

    return current


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
    locks[1] = normalize_approved_ca_updates(locks[1], locks[0])
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
    base = os.environ.get("MYPROXY_BOUNDARY_BASE", "67dd4c196d4f0e3408e3efdd426e6baafbfbf79102")
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
