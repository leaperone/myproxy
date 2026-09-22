#!/usr/bin/env python3
"""Check the original release workflows and sources that must stay unchanged."""
import os
import re
import subprocess
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
base = os.environ.get("MYPROXY_BOUNDARY_BASE", "67dd4c196d4f0e3408e3efdd426e6baafbf79102")
protected = (
    ".github/workflows/release.yml", ".github/workflows/ci.yml",
    "scripts/release-macos.sh", "scripts/package-macos-app.sh",
    "scripts/fetch-mihomo.sh", "scripts/check-release-artifacts.py",
    "scripts/sparkle_previous_tags.py", "packaging/macos/Info.plist",
    "Cargo.lock",
)
changed = subprocess.check_output(["git", "diff", "--name-only", base, "--", *protected], cwd=ROOT, text=True)
assert not changed, "protected files changed: " + changed
backend = (ROOT / "src/backend.rs").read_text()
assert 'cfg!(feature = "xray-channel")' in backend
assert "fs::" not in backend, "backend cannot be changed by sharing a runtime config"
updates = (ROOT / "src/updates.rs").read_text()
for feed in ("latest/download/appcast.xml", "download/nightly/appcast.xml", "download/xray/appcast.xml"):
    assert "https://github.com/leaperone/myproxy/releases/" + feed in updates
print("original build/release paths preserved; Xray requires a separate build feature")

compiler = (ROOT / "src/compile.rs").read_text()
compiler = re.sub(r"(?m)^[ \t]*unavailable_fallback: Default::default\(\),\n", "", compiler)
original = subprocess.check_output(["git", "show", base + ":src/compile.rs"], cwd=ROOT, text=True)
assert compiler == original, "default compiler behavior changed"
