#!/usr/bin/env python3
import unittest

from xray_release_metadata import release_metadata, validate_release


class ReleaseMetadataTests(unittest.TestCase):
    def test_same_base_and_channel_suffix_as_nightly(self):
        values = release_metadata("0.0.10", "20260922", 71, 1)
        self.assertEqual(values, {
            "version": "0.0.10-xray.20260922.71.1",
            "tag": "v0.0.10-xray.20260922.71.1",
            "build_number": "71.1",
            "archive": "myproxy-0.0.10-xray.20260922.71.1.sparkle.zip",
            "title": "myproxy Xray 0.0.10-xray.20260922.71.1",
        })
        self.assertEqual(release_metadata("0.0.11", "20261001", 72, 2)["version"], "0.0.11-xray.20261001.72.2")

    def test_bundle_and_feed_cannot_mix_version_schemes(self):
        version = "0.0.10-xray.20260922.71.1"
        archive = f"myproxy-{version}.sparkle.zip"
        self.assertEqual(validate_release(version, "71.1", f"v{version}", archive, "0.0.10")["tag"], f"v{version}")
        for build, tag, name, base in [
            ("1.6.71.1", f"v{version}", archive, "0.0.10"),
            ("71.1", f"xray-v{version}", archive, "0.0.10"),
            ("71.1", f"v{version}", f"myproxy-xray-{version}.sparkle.zip", "0.0.10"),
            ("71.1", f"v{version}", archive, "0.0.11"),
        ]:
            with self.assertRaises(ValueError):
                validate_release(version, build, tag, name, base)
        for version in ("1.6.1", "1.6.0-xray.8", "0.0.10-nightly.20260922.71.1"):
            with self.assertRaises(ValueError):
                validate_release(version, "71.1", f"v{version}", f"myproxy-{version}.sparkle.zip")

    def test_invalid_metadata_is_rejected(self):
        for base, date, run, attempt in [
            ("0.0.10-xray", "20260922", 71, 1),
            ("0.0.10", "20260230", 71, 1),
            ("0.0.10", "20260922", 0, 1),
            ("0.0.10", "20260922", 71, 0),
        ]:
            with self.assertRaises(ValueError):
                release_metadata(base, date, run, attempt)


if __name__ == "__main__":
    unittest.main()
