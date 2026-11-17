#!/usr/bin/env python3
"""Replay the pinned Window WPT selection against Rustic's headless host."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates/rustic-cli/tests/fixtures/wpt-web"
UPSTREAM = FIXTURES / "upstream"
MANIFEST = FIXTURES / "manifest.json"
EXPECTATIONS = FIXTURES / "expectations.json"
HARNESS = "resources/testharness.js"
LICENSE = "LICENSE.md"
REPORT_ATTRIBUTE = re.compile(r'local="data-rustic-wpt-output" value="([0-9a-f]+)"')
STATUS = ("PASS", "FAIL", "TIMEOUT", "NOTRUN", "PRECONDITION_FAILED")
ANY_PAGE = (
    b"<!doctype html><meta charset=utf-8>"
    b"<script src=testharness.js></script>"
    b"<script src=report.js></script>"
    b"<script src=case.js></script>"
)


def pinned_cases() -> list[str]:
    """Verify that the selected upstream files are complete and unmodified."""
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    cases = manifest["cases"]
    actual = {
        path.relative_to(UPSTREAM).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(UPSTREAM.rglob("*"))
        if path.is_file()
    }
    if not cases or sorted(set(cases)) != cases or {*cases, HARNESS, LICENSE} != actual.keys():
        raise ValueError("the manifest must list each selected test and its supporting files")
    if actual != manifest["sha256"]:
        raise ValueError("a pinned upstream file differs from the manifest")
    if not re.fullmatch(r"[0-9a-f]{40}", manifest["upstream_revision"]):
        raise ValueError("the upstream revision must be an exact Git commit")
    return cases


def read_expectations(cases: list[str]) -> dict:
    expected = json.loads(EXPECTATIONS.read_text(encoding="utf-8"))
    validate_expectations(expected, cases)
    return expected


def validate_expectations(expected: dict, cases: list[str]) -> None:
    if list(expected) != cases:
        raise ValueError("expectations must cover the pinned selection in order")
    for name, records in expected.items():
        if not records:
            raise ValueError(f"{name}: empty expectation list")
        for record in records:
            if record["status"] not in STATUS or not isinstance(record["name"], str):
                raise ValueError(f"{name}: invalid subtest expectation")
            if record["status"] != "PASS" and not record.get("message"):
                raise ValueError(f"{name}: every expected failure needs a reason")


def window_page(path: Path, directory: Path) -> bytes:
    source = path.read_bytes()
    if path.name.endswith(".any.js"):
        # This selection uses the ordinary Window variant without script,
        # timeout or globals directives. Fail if a future import needs one.
        for line in source.splitlines():
            if line.startswith(b"// META:") and not line.startswith(b"// META: title="):
                raise ValueError(f"{path.name}: unsupported META directive {line!r}")
        (directory / "case.js").write_bytes(source)
        return ANY_PAGE
    if path.suffix != ".html":
        raise ValueError(f"{path.name}: unrecognized WPT fixture")
    for before, after in (
        (b"/resources/testharness.js", b"testharness.js"),
        (b"/resources/testharnessreport.js", b"report.js"),
    ):
        if source.count(before) != 1:
            raise ValueError(f"{path.name}: expected exactly one reference to {before!r}")
        source = source.replace(before, after)
    return source


def run_case(binary: Path, name: str, directory: Path) -> list[dict]:
    page = directory / "case.html"
    page.write_bytes(window_page(UPSTREAM / name, directory))
    result = subprocess.run(
        [str(binary), "dump-dom", "--run-scripts", str(page)],
        check=False, capture_output=True, encoding="utf-8", timeout=30,
    )
    if result.returncode:
        raise ValueError(f"{name}: headless load failed: {result.stderr.strip()}")
    reports = REPORT_ATTRIBUTE.findall(result.stdout)
    if len(reports) != 1:
        raise ValueError(f"{name}: upstream harness did not send one completion report")
    report = json.loads(bytes.fromhex(reports[0]).decode("utf-16be"))
    if report["status"] != 0 or not report["tests"]:
        raise ValueError(f"{name}: harness status {report['status']}: {report['message']}")
    records = []
    for item in report["tests"]:
        number = item["status"]
        if not isinstance(number, int) or not 0 <= number < len(STATUS):
            raise ValueError(f"{name}: invalid subtest status {number!r}")
        status = STATUS[number]
        record = {"name": item["name"], "status": status}
        if status != "PASS":
            record["message"] = item["message"]
        records.append(record)
    return records


def run(binary: Path, cases: list[str]) -> dict:
    with tempfile.TemporaryDirectory(prefix="rustic-wpt-") as temp:
        directory = Path(temp)
        shutil.copyfile(UPSTREAM / HARNESS, directory / "testharness.js")
        shutil.copyfile(FIXTURES / "report.js", directory / "report.js")
        return {name: run_case(binary, name, directory) for name in cases}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rustic", type=Path, help="path to the built CLI executable")
    parser.add_argument("--record", action="store_true", help="replace the baseline for review")
    args = parser.parse_args()
    if args.record and args.rustic is None:
        parser.error("--record needs --rustic")
    try:
        cases = pinned_cases()
        expected = None if args.record else read_expectations(cases)
        if args.rustic is None:
            print(f"Verified pinned assets and expectations for {len(cases)} Window WPT files")
            return 0
        actual = run(args.rustic.resolve(strict=True), cases)
        validate_expectations(actual, cases)
        if args.record:
            EXPECTATIONS.write_text(json.dumps(actual, indent=2) + "\n", encoding="utf-8")
        else:
            changed = [name for name in cases if actual[name] != expected[name]]
            if changed:
                for name in changed:
                    print(f"{name}: expected {expected[name]!r}\n  actual {actual[name]!r}", file=sys.stderr)
                return 1
        counts = Counter(item["status"] for records in actual.values() for item in records)
        statuses = ", ".join(f"{counts[name]} {name}" for name in STATUS if counts[name])
        print(f"{len(cases)} Window WPT files, {counts.total()} subtests: {statuses}")
    except (OSError, ValueError, KeyError, IndexError, TypeError, subprocess.TimeoutExpired) as exc:
        print(f"Window WPT: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
