#!/usr/bin/env python3
"""Pick one Developer ID Application hash from `security find-identity` output."""

import re
import sys

LINE = re.compile(
    r"^\s*\d+\)\s+([0-9A-F]{40})\s+\"Developer ID Application:"
)


def hashes_from(text: str) -> list[str]:
    found: list[str] = []
    for line in text.splitlines():
        match = LINE.match(line)
        if match and match.group(1) not in found:
            found.append(match.group(1))
    return found


def select(text: str) -> str:
    found = hashes_from(text)
    if not found:
        return ""
    if len(found) > 1:
        sys.stderr.write(
            "multiple Developer ID Application identities; set CODESIGN_IDENTITY to one hash:\n"
        )
        for identity in found:
            sys.stderr.write(f"{identity}\n")
        raise SystemExit(1)
    return found[0]


def self_test() -> None:
    one = (
        '  1) F0798A9E5901C7CC0C69BC7A4B96A438E72E8E44 '
        '"Developer ID Application: Example (TEAMID)"\n'
    )
    if select(one) != "F0798A9E5901C7CC0C69BC7A4B96A438E72E8E44":
        raise SystemExit("single-identity pick failed")
    two = one + (
        '  2) ABCDEF0123456789ABCDEF0123456789ABCDEF01 '
        '"Developer ID Application: Example (TEAMID)"\n'
    )
    try:
        select(two)
    except SystemExit as error:
        if error.code != 1:
            raise SystemExit("multiple-identity exit code failed")
    else:
        raise SystemExit("multiple identities must fail")
    if select("     0 valid identities found\n") != "":
        raise SystemExit("empty identities must return no hash")
    print("select_developer_id_identity ok")


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        self_test()
        raise SystemExit(0)
    print(select(sys.stdin.read()))
