#!/usr/bin/env python3
"""Vendor and verify the pinned html5lib tree-construction corpus, offline."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates/rustic-html/tests/fixtures"
CORPUS = FIXTURES / "tree"
MANIFEST = CORPUS / "manifest.json"
RUNNER = ROOT / "crates/rustic-html/tests/tree_construction.rs"
REVISION = "9329e64694e7835d0dcff9811e22856ef6ad16f9"
HEADERS = {"#errors", "#new-errors", "#document", "#document-fragment", "#script-on", "#script-off"}


def records(source: str) -> list[dict[str, str]]:
    if not source.startswith("#data\n"):
        raise ValueError("expected a #data record")
    result = []
    for block in source.split("#data\n")[1:]:
        fields: dict[str, list[str]] = {"#data": []}
        current = "#data"
        for line in block.splitlines(keepends=True):
            header = line.rstrip("\n")
            if header in HEADERS:
                if header in fields:
                    raise ValueError(f"duplicate {header} section")
                current = header
                fields[current] = []
            else:
                fields[current].append(line)
        if "#errors" not in fields or "#document" not in fields:
            raise ValueError("record lacks errors/document section")
        values = {key: "".join(lines) for key, lines in fields.items()}
        # Each field has a delimiter newline; tree records also have an empty
        # separating line. Preserve actual newlines in the Unicode input field.
        values["#data"] = values["#data"].removesuffix("\n")
        values["#document"] = values["#document"].rstrip("\n") + "\n"
        result.append(values)
    return result


def exclusion(case: dict[str, str]) -> str | None:
    """Mirror the Rust runner's skip rule; it never depends on parser results."""
    if "<?" in case["#data"]:
        return "processing_instruction"
    return None


def manifest() -> str:
    files = []
    for path in sorted(CORPUS.glob("*.dat")):
        raw = path.read_bytes()
        cases = records(raw.decode("utf-8"))
        excluded = Counter(reason for reason in map(exclusion, cases) if reason)
        files.append({
            "path": path.name,
            "sha256": hashlib.sha256(raw).hexdigest(),
            "upstream_cases": len(cases),
            "runnable_cases": len(cases) - sum(excluded.values()),
            "exclusion_reasons": dict(sorted(excluded.items())),
        })
    metadata = {
        "upstream_repository": "https://github.com/html5lib/html5lib-tests",
        "upstream_revision": REVISION,
        "upstream_path": "tree-construction/*.dat (scripted/ not vendored)",
        "license": "MIT; ../HTML5LIB-LICENSE.md",
        "selection_policy": "all-document-and-fragment-records-with-scripting-mode-no-processing-instructions-v3",
        "files": files,
        "upstream_cases": sum(f["upstream_cases"] for f in files),
        "runnable_cases": sum(f["runnable_cases"] for f in files),
        "checked": ["complete tree, namespaces, attributes and template contents", "every two-chunk byte split", "one-byte feeds"],
        "not_checked": ["upstream parse errors/positions", "compatibility mode", "processing-instruction records"],
        "input_adapter": "Unicode fixture stream encoded as UTF-8 after an explicit transport BOM",
    }
    return json.dumps(metadata, indent=2, sort_keys=True) + "\n"


def check() -> None:
    if MANIFEST.read_text(encoding="utf-8") != manifest():
        raise ValueError("tree corpus differs from manifest; re-vendor deliberately")
    if not (FIXTURES / "HTML5LIB-LICENSE.md").is_file():
        raise ValueError("missing html5lib data license")
    runner = RUNNER.read_text(encoding="utf-8")
    for path in CORPUS.glob("*.dat"):
        if f'"{path.name}"' not in runner:
            raise ValueError(f"tree runner does not include {path.name}")
    if REVISION not in runner:
        raise ValueError("tree runner does not record the pinned revision")


def vendor(source: Path) -> None:
    """Copy every top-level .dat file from a local checkout at REVISION."""
    directory = source / "tree-construction"
    names = sorted(path.name for path in directory.glob("*.dat"))
    if not names:
        raise ValueError(f"no tree-construction data under {source}")
    for old in CORPUS.glob("*.dat"):
        old.unlink()
    for name in names:
        shutil.copyfile(directory / name, CORPUS / name)
    MANIFEST.write_text(manifest(), encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--check", action="store_true", help="verify vendored files")
    group.add_argument("--vendor", type=Path, metavar="CHECKOUT", help=f"copy from html5lib-tests at {REVISION}")
    args = parser.parse_args()
    if args.check:
        check()
    else:
        vendor(args.vendor)
    metadata = json.loads(MANIFEST.read_text())
    print(f"Tree corpus: {metadata['runnable_cases']}/{metadata['upstream_cases']} runnable records")


if __name__ == "__main__":
    main()
