#!/usr/bin/env python3
"""Run xcrun stapler validate with bounded retries on transient Apple network errors."""
from __future__ import annotations

import re
import subprocess
import sys
import time
from pathlib import Path
from typing import Callable, Sequence

STAPLER_VALIDATE_CMD = ("xcrun", "stapler", "validate")

# NSURLErrorDomain codes that indicate transient connectivity (not ticket/signature faults).
_TRANSIENT_NSURL_CODES = frozenset({"-1001", "-1003", "-1005", "-1009"})

_TRANSIENT_NETWORK_MARKERS = (
    "The network connection was lost",
    "The Internet connection appears to be offline",
    "A server with the specified hostname could not be found",
    "The request timed out",
    "Network is down",
)


def stapler_output_is_transient_network_failure(stdout: str, stderr: str) -> bool:
    combined = f"{stdout}\n{stderr}"
    if "NSURLErrorDomain" not in combined:
        return False
    if any(code in _TRANSIENT_NSURL_CODES for code in re.findall(r"\bCode\s*=\s*(-\d+)\b", combined)):
        return True
    return any(marker in combined for marker in _TRANSIENT_NETWORK_MARKERS)


def run_stapler_validate(
    app_path: str | Path,
    *,
    runner: Callable[[Sequence[str]], subprocess.CompletedProcess[str]] | None = None,
    sleeper: Callable[[float], None] | None = None,
    max_attempts: int = 5,
    initial_backoff_seconds: float = 2.0,
) -> None:
    path = Path(app_path)
    cmd = [*STAPLER_VALIDATE_CMD, str(path)]
    run = runner or (
        lambda arguments: subprocess.run(
            list(arguments),
            capture_output=True,
            text=True,
            check=False,
        )
    )
    sleep = sleeper or time.sleep

    last: subprocess.CompletedProcess[str] | None = None
    for attempt in range(1, max_attempts + 1):
        last = run(cmd)
        if last.returncode == 0:
            return
        out = last.stdout or ""
        err = last.stderr or ""
        if last.returncode != 68 or not stapler_output_is_transient_network_failure(out, err):
            _report_and_exit(cmd, last)
        if attempt < max_attempts:
            delay = initial_backoff_seconds * attempt
            print(f"stapler network failure, attempt {attempt}/{max_attempts}; retry in {delay:g}s", file=sys.stderr)
            sleep(delay)
    assert last is not None
    _report_and_exit(cmd, last)


def _report_and_exit(cmd: Sequence[str], result: subprocess.CompletedProcess[str]) -> None:
    print(f"stapler validate failed (exit {result.returncode})", file=sys.stderr)
    print(f"command: {list(cmd)}", file=sys.stderr)
    if result.stdout:
        print("stdout:", file=sys.stderr)
        print(result.stdout, file=sys.stderr, end="" if result.stdout.endswith("\n") else "\n")
    if result.stderr:
        print("stderr:", file=sys.stderr)
        print(result.stderr, file=sys.stderr, end="" if result.stderr.endswith("\n") else "\n")
    sys.exit(result.returncode if result.returncode else 1)


def main() -> None:
    if len(sys.argv) != 2:
        print(f"usage: {Path(__file__).name} <path-to-app>", file=sys.stderr)
        sys.exit(2)
    run_stapler_validate(sys.argv[1])


if __name__ == "__main__":
    main()
