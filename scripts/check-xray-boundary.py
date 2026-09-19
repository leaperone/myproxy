#!/usr/bin/env python3
"""Check the original release workflows and sources that must stay unchanged."""
import os
import subprocess
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
base = os.environ.get("MYPROXY_BOUNDARY_BASE", "67dd4c196d4f0e3408e3efdd426e6baafbf79102")
protected = (
    ".github/workflows/release.yml", ".github/workflows/ci.yml",
    "scripts/release-macos.sh", "scripts/package-macos-app.sh",
    "scripts/fetch-mihomo.sh", "scripts/check-release-artifacts.py",
    "scripts/sparkle_previous_tags.py", "packaging/macos/Info.plist",
    "src/compile.rs", "src/updates.rs", "Cargo.lock",
)
changed = subprocess.check_output(["git", "diff", "--name-only", base, "--", *protected], cwd=ROOT, text=True)
assert not changed, "protected files changed: " + changed
backend = (ROOT / "src/backend.rs").read_text()
assert 'cfg!(feature = "xray-channel")' in backend
assert "fs::" not in backend, "backend cannot be changed by sharing a runtime config"
print("original build/release paths preserved; Xray requires a separate build feature")
