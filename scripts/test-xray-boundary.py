#!/usr/bin/env python3
import importlib.util
import plistlib
import os
import sys
import subprocess
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("boundary", Path(__file__).with_name("check-xray-boundary.py"))
boundary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(boundary)

ROOT = Path(__file__).resolve().parents[1]
XRAY_BASE = "87f1f87c775d8899878d87ae8ff7656009151b15"

LOCK = '''version = 4
[[package]]
name = "myproxy"
version = "0.0.10"
dependencies = ["serde"]
[[package]]
name = "serde"
version = "1.0.0"
source = "registry+fixture"
checksum = "unchanged"
'''


def metadata(version):
    return {"host.plist": plistlib.dumps({"CFBundleIdentifier": "local.harry.myproxy", "CFBundleShortVersionString": version, "CFBundleVersion": version})}


class BoundaryTests(unittest.TestCase):
    def verify(self, lock=None, plists=None):
        boundary.verify_version_metadata(LOCK, lock or LOCK.replace('version = "0.0.10"', 'version = "0.0.11"'), metadata("0.0.10"), plists or metadata("0.0.11"), "0.0.11")

    def test_synchronized_patch_is_allowed(self):
        self.verify()

    def test_dependency_change_is_rejected(self):
        lock = LOCK.replace('version = "0.0.10"', 'version = "0.0.11"').replace('version = "1.0.0"', 'version = "1.0.1"')
        with self.assertRaisesRegex(AssertionError, "dependency data changed"):
            self.verify(lock=lock)

    def test_stale_lock_version_is_rejected(self):
        with self.assertRaisesRegex(AssertionError, "does not match"):
            self.verify(lock=LOCK)

    def test_bundle_identity_change_is_rejected(self):
        plists = metadata("0.0.11")
        data = plistlib.loads(plists["host.plist"])
        data["CFBundleIdentifier"] = "unexpected.bundle"
        plists["host.plist"] = plistlib.dumps(data)
        with self.assertRaisesRegex(AssertionError, "protected packaging metadata changed"):
            self.verify(plists=plists)

    def test_stale_bundle_version_is_rejected(self):
        with self.assertRaisesRegex(AssertionError, "does not match"):
            self.verify(plists=metadata("0.0.10"))


class ApprovedCaLockTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.baseline_lock = subprocess.check_output(["git", "show", f"{XRAY_BASE}:Cargo.lock"], cwd=ROOT).decode()
        cls.current_lock = (ROOT / "Cargo.lock").read_text()
        cls.version = __import__("tomllib").loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
        cls.plists = ("packaging/macos/Info.plist", "packaging/macos/NetworkExtension/Info.plist")
        cls.baseline_plists = {
            name: subprocess.check_output(["git", "show", f"{XRAY_BASE}:{name}"], cwd=ROOT)
            for name in cls.plists
        }
        cls.current_plists = {name: (ROOT / name).read_bytes() for name in cls.plists}

    def verify_lock(self, original_lock, current_lock):
        boundary.verify_version_metadata(
            original_lock,
            current_lock,
            self.baseline_plists,
            self.current_plists,
            self.version,
        )

    def test_full_boundary_command_accepts_reviewed_ca_update(self):
        env = os.environ.copy()
        env.pop("MYPROXY_BOUNDARY_BASE", None)
        result = subprocess.run(
            [sys.executable, str(ROOT / "scripts/check-xray-boundary.py")],
            cwd=ROOT, env=env, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_original_xray_lock_is_allowed(self):
        self.verify_lock(self.baseline_lock, self.baseline_lock)

    def test_exact_ca_patch_is_allowed(self):
        self.verify_lock(self.baseline_lock, self.current_lock)

    def test_unrelated_tempfile_edge_is_rejected(self):
        tampered = self.current_lock.replace('"getrandom 0.4.3"', '"getrandom 0.3.4"', 1)
        with self.assertRaisesRegex(AssertionError, "dependency data changed"):
            self.verify_lock(self.baseline_lock, tampered)

    def test_tampered_ca_checksum_is_rejected(self):
        tampered = self.current_lock.replace(
            "4443b1b6d39d7c663139942cc4558262562d2c84123bb6282ef177009ca8b281",
            "4443b1b6d39d7c663139942cc4558262562d2c84123bb6282ef177009ca8b2810",
            1,
        )
        with self.assertRaisesRegex(AssertionError, "checksum does not match approved CA update"):
            self.verify_lock(self.baseline_lock, tampered)

    def test_unapproved_ca_version_is_rejected(self):
        tampered = self.current_lock.replace('version = "0.103.16"', 'version = "0.103.17"', 1)
        with self.assertRaisesRegex(AssertionError, "dependency data changed"):
            self.verify_lock(self.baseline_lock, tampered)

    def test_tampered_ca_source_is_rejected(self):
        tampered = self.current_lock.replace(
            'name = "rustls-webpki"\nversion = "0.103.16"\nsource = "registry+https://github.com/rust-lang/crates.io-index"',
            'name = "rustls-webpki"\nversion = "0.103.16"\nsource = "registry+https://example.com/index"',
            1,
        )
        with self.assertRaisesRegex(AssertionError, "source must stay official registry"):
            self.verify_lock(self.baseline_lock, tampered)

    def test_transitive_ca_dependency_change_is_rejected(self):
        tampered = self.current_lock.replace(
            'name = "rustls-webpki"\nversion = "0.103.16"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n'
            'checksum = "4443b1b6d39d7c663139942cc4558262562d2c84123bb6282ef177009ca8b281"\n'
            'dependencies = [\n "aws-lc-rs",\n "ring",\n "rustls-pki-types",\n "untrusted",\n]',
            'name = "rustls-webpki"\nversion = "0.103.16"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n'
            'checksum = "4443b1b6d39d7c663139942cc4558262562d2c84123bb6282ef177009ca8b281"\n'
            'dependencies = [\n "aws-lc-rs",\n "ring",\n "rustls-pki-types",\n "serde",\n "untrusted",\n]',
            1,
        )
        with self.assertRaisesRegex(AssertionError, "dependency graph changed beyond approved CA update"):
            self.verify_lock(self.baseline_lock, tampered)


if __name__ == "__main__":
    unittest.main()
