#!/usr/bin/env python3
"""Dependency-free source/documentation consistency checks for Python 3.11+."""

from pathlib import Path
import json
import re
import sys
import tomllib
from urllib.parse import unquote, urlsplit


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
