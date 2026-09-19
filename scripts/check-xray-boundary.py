#!/usr/bin/env python3
"""Check that the Xray test channel stays outside the existing release path."""

from pathlib import Path
import subprocess
import os


ROOT = Path(__file__).resolve().parents[1]


def run(*args: str) -> str:
    return subprocess.check_output(args, cwd=ROOT, text=True)


protected = (
    ".github/workflows/release.yml",
    "scripts/release-macos.sh",
    "scripts/package-macos-app.sh",
    "scripts/fetch-mihomo.sh",
    "scripts/check-release-artifacts.py",
)
base = os.environ.get("MYPROXY_BOUNDARY_BASE", "origin/main")
changed = run("git", "diff", "--name-only", base, "--", *protected).splitlines()
if changed:
    raise SystemExit("Xray work changed protected release files: " + ", ".join(changed))

workflow = (ROOT / ".github/workflows/release.yml").read_text()
if "fetch-xray.sh" in workflow or "package-xray-test.sh" in workflow:
    raise SystemExit("the existing release workflow must not package Xray")

backend = (ROOT / "src/backend.rs").read_text()
if "#[default]\n    Mihomo" not in backend:
    raise SystemExit("Mihomo is no longer the default backend")

xray = (ROOT / "src/xray.rs").read_text()
if 'insert("allowInsecure"' in xray or '"allowInsecure":' in xray:
    raise SystemExit("Xray projection must not emit allowInsecure")

print("protected release path unchanged; Mihomo remains the default backend")
