#!/usr/bin/env python3
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from stapler_validate import (
    run_stapler_validate,
    stapler_output_is_transient_network_failure,
)


def _completed(returncode: int, stdout: str = "", stderr: str = "") -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(args=[], returncode=returncode, stdout=stdout, stderr=stderr)


class TransientDetectionTests(unittest.TestCase):
    def test_nsurl_transient_codes(self):
        err = "NSURLErrorDomain Code=-1005 'The network connection was lost.'"
        self.assertTrue(stapler_output_is_transient_network_failure("", err))

    def test_other_error_domain_is_not_retryable(self):
        self.assertFalse(stapler_output_is_transient_network_failure("OtherDomain Code=-1005", ""))
        self.assertFalse(stapler_output_is_transient_network_failure("NSURLErrorDomain Code=-10050", ""))

    def test_permanent_validate_failure(self):
        err = "validate failed Error68\nThe staple and notarization ticket are missing."
        self.assertFalse(stapler_output_is_transient_network_failure("", err))


class RunStaplerValidateTests(unittest.TestCase):
    def test_success_on_first_attempt(self):
        calls: list[list[str]] = []

        def runner(cmd):
            calls.append(list(cmd))
            return _completed(0)

        run_stapler_validate("/tmp/Fake.app", runner=runner, sleeper=lambda _: None, max_attempts=3)
        self.assertEqual(len(calls), 1)

    def test_transient_then_success(self):
        responses = [
            _completed(
                68,
                stderr="NSURLErrorDomain Code=-1005 'The network connection was lost.'\n",
            ),
            _completed(0),
        ]
        sleeps: list[float] = []

        def runner(_cmd):
            return responses.pop(0)

        run_stapler_validate(
            "/tmp/Fake.app",
            runner=runner,
            sleeper=sleeps.append,
            max_attempts=5,
            initial_backoff_seconds=1.0,
        )
        self.assertEqual(len(sleeps), 1)
        self.assertEqual(sleeps[0], 1.0)

    def test_permanent_failure_no_retry(self):
        calls = 0

        def runner(_cmd):
            nonlocal calls
            calls += 1
            return _completed(68, stderr="The staple and notarization ticket are missing.")

        with self.assertRaises(SystemExit) as ctx:
            run_stapler_validate("/tmp/Fake.app", runner=runner, sleeper=lambda _: None, max_attempts=5)
        self.assertEqual(calls, 1)
        self.assertEqual(ctx.exception.code, 68)

    def test_transient_exhausted_fails(self):
        err = "NSURLErrorDomain Code=-1009 'The Internet connection appears to be offline.'\n"
        calls = 0

        def runner(_cmd):
            nonlocal calls
            calls += 1
            return _completed(68, stderr=err)

        with self.assertRaises(SystemExit) as ctx:
            run_stapler_validate(
                "/tmp/Fake.app",
                runner=runner,
                sleeper=lambda _: None,
                max_attempts=3,
            )
        self.assertEqual(calls, 3)
        self.assertEqual(ctx.exception.code, 68)


if __name__ == "__main__":
    unittest.main()
