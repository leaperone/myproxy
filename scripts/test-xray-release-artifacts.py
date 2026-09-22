#!/usr/bin/env python3
import hashlib
import importlib.util
import plistlib
import tempfile
import unittest
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

spec = importlib.util.spec_from_file_location("publish_xray", Path(__file__).with_name("publish-xray-release.py"))
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


class ArtifactTests(unittest.TestCase):
    def test_verified_metadata_and_mismatched_feed_or_archive(self):
        version = "0.0.10-xray.20260922.71.1"
        tag = "v" + version
        name = f"myproxy-{version}.sparkle.zip"
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            archive = directory / name
            with zipfile.ZipFile(archive, "w") as bundle:
                bundle.writestr("MyProxy.app/Contents/Info.plist", plistlib.dumps({
                    "CFBundleShortVersionString": version, "CFBundleVersion": "71.1", "MyproxyBuildChannel": "xray",
                }))
            checksum = directory / (name + ".sha256")
            checksum.write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + "  " + name + "\n")
            root = ET.Element("rss")
            item = ET.SubElement(ET.SubElement(root, "channel"), "item")
            ET.SubElement(item, publisher.SPARKLE + "channel").text = "xray"
            ET.SubElement(item, publisher.SPARKLE + "version").text = "71.1"
            ET.SubElement(item, publisher.SPARKLE + "shortVersionString").text = version
            enclosure = ET.SubElement(item, "enclosure", {
                "url": f"https://github.com/leaperone/myproxy/releases/download/{tag}/{name}",
                "length": str(archive.stat().st_size), publisher.SPARKLE + "edSignature": "signed-build-fixture",
            })
            appcast = directory / "appcast.xml"
            ET.ElementTree(root).write(appcast)
            verify = lambda: publisher.verify_artifacts(directory, version, "71.1", tag, name, "leaperone/myproxy")
            self.assertEqual([path.name for path in verify()], [name, "appcast.xml", name + ".sha256"])
            enclosure.set("url", f"https://github.com/leaperone/myproxy/releases/download/nightly/{name}")
            ET.ElementTree(root).write(appcast)
            with self.assertRaisesRegex(ValueError, "different release"):
                verify()
            enclosure.set("url", f"https://github.com/leaperone/myproxy/releases/download/{tag}/{name}")
            item.find(publisher.SPARKLE + "shortVersionString").text = "1.6.1"
            ET.ElementTree(root).write(appcast)
            with self.assertRaisesRegex(ValueError, "display version"):
                verify()
            checksum.write_text("0" * 64 + "  " + name + "\n")
            with self.assertRaisesRegex(ValueError, "checksum"):
                verify()


if __name__ == "__main__":
    unittest.main()
