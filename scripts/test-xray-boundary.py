#!/usr/bin/env python3
import importlib.util
import plistlib
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("boundary", Path(__file__).with_name("check-xray-boundary.py"))
boundary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(boundary)

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


if __name__ == "__main__":
    unittest.main()
