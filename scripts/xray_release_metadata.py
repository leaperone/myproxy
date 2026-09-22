#!/usr/bin/env python3
"""Use the same package/date/run/attempt version scheme as Nightly."""
import argparse
import datetime
import json
import re
import tomllib
from pathlib import Path


def release_metadata(package_version, date, run, attempt):
    if not re.fullmatch(r"\d+\.\d+\.\d+", package_version):
        raise ValueError("Cargo.toml must contain a MAJOR.MINOR.PATCH version")
    datetime.datetime.strptime(date, "%Y%m%d")
    if len(date) != 8 or run < 1 or attempt < 1:
        raise ValueError("release date, run number, or attempt is invalid")
    version = f"{package_version}-xray.{date}.{run}.{attempt}"
    return {
        "version": version,
        "tag": f"v{version}",
        "build_number": f"{run}.{attempt}",
        "archive": f"myproxy-{version}.sparkle.zip",
        "title": f"myproxy Xray {version}",
    }


def validate_release(version, build_number, tag, archive, package_version=None):
    match = re.fullmatch(r"(\d+\.\d+\.\d+)-xray\.(\d{8})\.(\d+)\.(\d+)", version)
    if not match:
        raise ValueError("Xray version must be MAJOR.MINOR.PATCH-xray.YYYYMMDD.RUN.ATTEMPT")
    base, date, run, attempt = match.groups()
    expected = release_metadata(base, date, int(run), int(attempt))
    if package_version is not None and base != package_version:
        raise ValueError("Xray base version must match Cargo.toml")
    if (version, build_number, tag, archive) != (
        expected["version"], expected["build_number"], expected["tag"], expected["archive"]
    ):
        raise ValueError("Xray version, tag, build number, and archive name must agree")
    return expected


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--date", default=datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%d"))
    parser.add_argument("--run", type=int, required=True)
    parser.add_argument("--attempt", type=int, required=True)
    parser.add_argument("--github-env", type=Path)
    parser.add_argument("--github-output", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    package_version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    values = release_metadata(package_version, args.date, args.run, args.attempt)
    if args.github_env:
        with args.github_env.open("a") as output:
            for key, name in (("version", "MYPROXY_XRAY_VERSION"), ("tag", "MYPROXY_RELEASE_TAG"), ("build_number", "MYPROXY_XRAY_BUILD_NUMBER")):
                output.write(f"{name}={values[key]}\n")
    if args.github_output:
        with args.github_output.open("a") as output:
            for key, value in values.items():
                output.write(f"{key}={value}\n")
    print(json.dumps(values))
