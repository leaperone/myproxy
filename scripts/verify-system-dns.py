#!/usr/bin/env python3
"""Compare native DNS/TLS with explicit Mixed proxy access to app backends."""
import argparse
import concurrent.futures
import json
import subprocess

ENDPOINTS = {
    "https://api2.cursor.sh/": 200,
    "https://api3.cursor.sh/": 404,
    "https://api.x.ai/v1/models": 401,
}


def probe(job):
    url, expected, mode, attempt, args = job
    command = [
        "curl", "--silent", "--show-error", "--output", "/dev/null",
        "--connect-timeout", str(args.timeout), "--max-time", str(args.timeout),
        "--write-out", "%{http_code}\t%{time_total}\t%{errormsg}",
    ]
    command += ["--noproxy", "*"] if mode == "system" else [
        "--noproxy", "", "--proxy", args.proxy,
    ]
    result = subprocess.run(command + [url], capture_output=True, text=True, timeout=args.timeout + 3)
    fields = result.stdout.split("\t", 2)
    status = int(fields[0]) if fields and fields[0].isdigit() else 0
    return {
        "url": url, "mode": mode, "attempt": attempt,
        "exit_code": result.returncode, "http_status": status,
        "seconds": float(fields[1]) if len(fields) > 1 else None,
        "error": fields[2] if len(fields) > 2 else result.stderr.strip(),
        "passed": result.returncode == 0 and status == expected,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--proxy", default="http://127.0.0.1:40808")
    parser.add_argument("--repeat", type=int, default=3)
    parser.add_argument("--timeout", type=int, default=12)
    args = parser.parse_args()
    if args.repeat < 1 or args.timeout < 1:
        parser.error("repeat and timeout must be positive")
    jobs = [(url, expected, mode, attempt, args)
            for attempt in range(1, args.repeat + 1)
            for url, expected in ENDPOINTS.items()
            for mode in ("system", "proxy")]
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as executor:
        results = list(executor.map(probe, jobs))
    print(json.dumps({"passed": all(row["passed"] for row in results), "results": results}, indent=2))
    return 0 if all(row["passed"] for row in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
