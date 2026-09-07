#!/usr/bin/env python3
"""Select previous same-channel Sparkle archive tags for generate_appcast."""

from __future__ import annotations

import argparse
import json
import sys


def select_previous_tags(releases, channel: str, current_tag: str, limit: int = 6) -> list[str]:
    tags: list[str] = []
    for release in releases:
        tag = release.get("tagName") or release.get("tag_name") or ""
        if not tag or tag == current_tag or tag == "nightly":
            continue
        prerelease = bool(release.get("isPrerelease", release.get("prerelease", False)))
        if channel == "prod" and prerelease:
            continue
        if channel == "nightly" and not prerelease:
            continue
        tags.append(tag)
        if len(tags) >= limit:
            break
    return tags


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("channel", choices=("prod", "nightly"))
    parser.add_argument("current_tag")
    parser.add_argument("--limit", type=int, default=6)
    args = parser.parse_args()
    raw = sys.stdin.read().strip()
    releases = json.loads(raw) if raw else []
    if not isinstance(releases, list):
        raise SystemExit("expected a JSON array of GitHub releases")
    for tag in select_previous_tags(releases, args.channel, args.current_tag, args.limit):
        print(tag)
    return 0


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--self-test":
        releases = [
            {"tagName": "nightly", "isPrerelease": True},
            {"tagName": "v0.0.7-nightly.20260907.20.1", "isPrerelease": True},
            {"tagName": "v0.0.6", "isPrerelease": False},
            {"tagName": "v0.0.7-nightly.20260906.19.1", "isPrerelease": True},
            {"tagName": "v0.0.5", "isPrerelease": False},
        ]
        assert select_previous_tags(releases, "nightly", "v0.0.7-nightly.20260907.21.1") == [
            "v0.0.7-nightly.20260907.20.1",
            "v0.0.7-nightly.20260906.19.1",
        ]
        assert select_previous_tags(releases, "prod", "v0.0.7") == ["v0.0.6", "v0.0.5"]
        assert select_previous_tags(releases, "nightly", "v0.0.7-nightly.20260907.20.1") == [
            "v0.0.7-nightly.20260906.19.1"
        ]
        print("ok")
        raise SystemExit(0)
    raise SystemExit(main())
