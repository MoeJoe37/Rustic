#!/usr/bin/env python3
"""Dependency-free source/documentation consistency checks for Python 3.11+."""

from pathlib import Path
import hashlib
import json
import re
import sys
import tomllib
from urllib.parse import unquote, urlsplit
import import_html_entities


ROOT = Path(__file__).resolve().parents[1]
IGNORED = {".git", "target", "artifacts", "__pycache__", "checkouts"}
LINK = re.compile(r"!?\[[^\]\n]*\]\(([^)\n]+)\)")
ERRORS: list[str] = []


def error(path: Path, detail: str) -> None:
    ERRORS.append(f"{path.relative_to(ROOT)}: {detail}")


def check_markdown(path: Path, source: str) -> None:
    in_fence = False
    for number, line in enumerate(source.splitlines(), 1):
        if line.lstrip().startswith(("```", "~~~")):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        for match in LINK.finditer(line):
            target = match.group(1).split()[0].strip("<>")
            if urlsplit(target).scheme or target.startswith(("#", "//")):
                continue
            local = unquote(target.split("#", 1)[0].split("?", 1)[0])
            resolved = (path.parent / local).resolve()
            if not resolved.is_relative_to(ROOT) or not resolved.exists():
                error(path, f"line {number}: missing local link: {target}")


def check_html_fixture(stem: str) -> None:
    manifest_path = ROOT / f"crates/rustic-html/tests/fixtures/{stem}_manifest.json"
    if not manifest_path.exists():
        return
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        fixture = manifest_path.parent / f"{stem}_cases.rs"
        if hashlib.sha256(fixture.read_bytes()).hexdigest() != manifest["fixture_sha256"]:
            error(fixture, "generated fixture digest differs from HTML manifest; regenerate deliberately")
        if not (manifest_path.parent / "HTML5LIB-LICENSE.md").is_file():
            error(manifest_path, "missing upstream html5lib license")
        if not re.fullmatch(r"[0-9a-f]{40}", manifest["upstream_revision"]):
            error(manifest_path, "upstream revision must be a full commit SHA")
        sources = manifest["sources"]
        for source in sources:
            indices = source["selected_array_indices"]
            if (len(indices) != source["selected_cases"] or indices != sorted(set(indices))
                    or any(index < 0 or index >= source["upstream_cases"] for index in indices)
                    or source["selected_cases"] + source["excluded_cases"] != source["upstream_cases"]
                    or sum(source["exclusion_reasons"].values()) != source["excluded_cases"]):
                error(manifest_path, f"inconsistent source counts/indices: {source['path']}")
        for count in ("upstream_cases", "selected_cases", "excluded_cases"):
            if sum(source[count] for source in sources) != manifest[count]:
                error(manifest_path, f"inconsistent aggregate {count}")
    except (OSError, KeyError, TypeError, ValueError) as exc:
        error(manifest_path, f"invalid HTML fixture metadata: {exc}")


def check_entities() -> None:
    data = ROOT / "crates/rustic-html/data"
    manifest_path = data / "entities_manifest.json"
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        rust, metadata = import_html_entities.generate((data / "entities.json").read_bytes(), manifest["data_version"])
        table = ROOT / "crates/rustic-html/src/entities.rs"
        if table.read_text(encoding="utf-8") != rust or manifest != json.loads(metadata):
            error(table, "entity source/table/manifest differ; run tools/import_html_entities.py deliberately")
        if not (data / "WHATWG-LICENSE.md").is_file():
            error(manifest_path, "missing WHATWG data notice")
    except (OSError, KeyError, TypeError, ValueError) as exc:
        error(manifest_path, f"invalid entity table/provenance: {exc}")


def main() -> int:
    files = [
        path
        for path in ROOT.rglob("*")
        if path.is_file()
        and not set(path.relative_to(ROOT).parts).intersection(IGNORED)
    ]
    for path in files:
        if path.name == "LICENSE":
            continue  # Preserve the upstream license verbatim.
        if path.suffix not in {".md", ".toml", ".rs", ".yml", ".yaml", ".py", ".json", ".lock"}:
            continue
        source = path.read_text(encoding="utf-8")
        if source and not source.endswith("\n"):
            error(path, "missing final newline")
        for number, line in enumerate(source.splitlines(), 1):
            if line != line.rstrip():
                error(path, f"line {number}: trailing whitespace")
        if path.suffix in {".toml", ".lock"}:
            try:
                tomllib.loads(source)
            except tomllib.TOMLDecodeError as exc:
                error(path, f"invalid TOML: {exc}")
        elif path.suffix == ".json":
            try:
                json.loads(source)
            except json.JSONDecodeError as exc:
                error(path, f"invalid JSON: {exc}")
        elif path.suffix == ".md":
            check_markdown(path, source)

    url_manifest = ROOT / "crates/rustic-url/tests/fixtures/wpt_manifest.json"
    if url_manifest.exists():
        try:
            manifest = json.loads(url_manifest.read_text(encoding="utf-8"))
            fixture = url_manifest.parent / "wpt_cases.rs"
            if hashlib.sha256(fixture.read_bytes()).hexdigest() != manifest["fixture_sha256"]:
                error(fixture, "generated fixture digest differs from WPT manifest; regenerate deliberately")
            if not (url_manifest.parent / "WPT-LICENSE.md").is_file():
                error(url_manifest, "missing upstream WPT license")
            selected = manifest["selected_cases"]
            indices = manifest["selected_array_indices"]
            if (manifest["positive_cases"] + manifest["negative_cases"] != selected
                    or selected + manifest["excluded_cases"] != manifest["upstream_cases"]
                    or len(indices) != selected or indices != sorted(set(indices))):
                error(url_manifest, "inconsistent WPT counts or selected indices")
        except (OSError, KeyError, TypeError, json.JSONDecodeError) as exc:
            error(url_manifest, f"invalid WPT fixture metadata: {exc}")

    check_html_fixture("html5lib")
    check_html_fixture("references")
    check_entities()

    try:
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
        toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]
        for member in workspace["members"]:
            if not (ROOT / member / "Cargo.toml").is_file():
                error(ROOT / "Cargo.toml", f"workspace member missing: {member}")
        workflow = ROOT / ".github/workflows/ci.yml"
        workflow_source = workflow.read_text()
        rust_pin = re.search(r'^  RUST_VERSION: "([^"]+)"$', workflow_source, re.MULTILINE)
        if rust_pin is None or rust_pin.group(1) != toolchain["channel"]:
            error(workflow, "RUST_VERSION must match rust-toolchain.toml")
        msrv = workspace["package"]["rust-version"]
        if f'"{msrv}.0"' not in workflow_source:
            error(workflow, "compatibility matrix must include workspace MSRV")
        for action in re.findall(r"uses:\s*(\S+)", workflow_source):
            if not re.fullmatch(r"[\w.-]+/[\w./-]+@[0-9a-f]{40}", action):
                error(workflow, f"action is not pinned to a full commit SHA: {action}")
    except (OSError, KeyError, tomllib.TOMLDecodeError) as exc:
        ERRORS.append(f"repository configuration: {exc}")

    if ERRORS:
        print("\n".join(ERRORS), file=sys.stderr)
        return 1
    print(f"Repository checks passed ({len(files)} source files inspected).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
