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
import import_html_tests
import import_html_tree_tests
import import_css_tests
import import_wpt_css_tests
import import_wpt_flexbox_tests


ROOT = Path(__file__).resolve().parents[1]
IGNORED = {".git", "target", "artifacts", "__pycache__", "checkouts"}
LINK = re.compile(r"!?\[[^\]\n]*\]\(([^)\n]+)\)")
ERRORS: list[str] = []
DEPENDENCY_KEYS = ("dependencies", "dev-dependencies", "build-dependencies")
DEBT_MARKER = re.compile(r"\b(TODO|FIXME|HACK|XXX)\b")
DEBT_ID = re.compile(r"\bTD-\d+\b")


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
            if "selected_states" in source:
                states = source["selected_states"]
                if (set(states) != {str(index) for index in indices}
                        or sum(map(len, states.values())) != source["selected_state_runs"]
                        or any(not modes or len(modes) != len(set(modes)) or not set(modes).issubset(import_html_tests.TEXT_MODES) for modes in states.values())):
                    error(manifest_path, f"inconsistent text-state runs: {source['path']}")
        for count in ("upstream_cases", "selected_cases", "excluded_cases"):
            if sum(source[count] for source in sources) != manifest[count]:
                error(manifest_path, f"inconsistent aggregate {count}")
        if "selected_state_runs" in manifest and sum(source["selected_state_runs"] for source in sources) != manifest["selected_state_runs"]:
            error(manifest_path, "inconsistent aggregate text-state runs")
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


def check_workflows(channel: str) -> None:
    """Every workflow uses the pinned toolchain and SHA-pinned actions (TD-07)."""
    for workflow in sorted((ROOT / ".github/workflows").glob("*.y*ml")):
        source = workflow.read_text(encoding="utf-8")
        pins = re.findall(r'^\s*RUST_VERSION: "([^"]+)"$', source, re.MULTILINE)
        if workflow.name == "ci.yml" and not pins:
            error(workflow, "RUST_VERSION must match rust-toolchain.toml")
        for pin in pins:
            if pin != channel:
                error(workflow, f"RUST_VERSION {pin} must match rust-toolchain.toml ({channel})")
        for action in re.findall(r"uses:\s*(\S+)", source):
            if not re.fullmatch(r"[\w.-]+/[\w./-]+@[0-9a-f]{40}", action):
                error(workflow, f"action is not pinned to a full commit SHA: {action}")


def check_dependencies(members: list[str]) -> None:
    """External crates need the docs/DEPENDENCIES.md review and an ADR first."""
    def tables(manifest: dict) -> list[dict]:
        found = [manifest.get(key, {}) for key in DEPENDENCY_KEYS]
        for target in manifest.get("target", {}).values():
            found.extend(target.get(key, {}) for key in DEPENDENCY_KEYS)
        return found

    review_path = ROOT / "third_party/rust-dependencies.json"
    review = {"direct": {}, "packages": []}
    if review_path.is_file():
        try:
            review = json.loads(review_path.read_text(encoding="utf-8"))
            if not (ROOT / review["decision"]).is_file():
                error(review_path, "missing dependency decision")
            for package in review["packages"]:
                if not package["notices"]:
                    error(review_path, f"missing license notices for {package['name']}")
                for name, digest in package["notices"].items():
                    if hashlib.sha256((ROOT / name).read_bytes()).hexdigest() != digest:
                        error(review_path, f"license notice digest changed: {name}")
        except (OSError, KeyError, TypeError, ValueError) as exc:
            error(review_path, f"invalid dependency review: {exc}")
            review = {"direct": {}, "packages": []}
    seen = set()
    for member in members:
        path = ROOT / member / "Cargo.toml"
        for table in tables(tomllib.loads(path.read_text(encoding="utf-8"))):
            for name, spec in table.items():
                if isinstance(spec, dict) and ("path" in spec or spec.get("workspace")):
                    continue
                key = member + ":" + name
                seen.add(key)
                if review["direct"].get(key) != spec:
                    error(path, f"external dependency {name!r} needs an exact reviewed entry in "
                          "docs/DEPENDENCIES.md, an ADR and third_party/rust-dependencies.json")
    if set(review["direct"]) != seen:
        error(review_path, "reviewed direct dependency set differs from manifests")
    workspace_deps = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"].get("dependencies", {})
    for name, spec in workspace_deps.items():
        if not (isinstance(spec, dict) and "path" in spec):
            error(ROOT / "Cargo.toml", f"external workspace dependency {name!r} is not reviewed")
    reviewed = {(p["name"], p["version"]): p for p in review["packages"]}
    actual = set()
    for package in tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]:
        if "source" not in package:
            continue
        key = (package["name"], package["version"])
        actual.add(key)
        expected = reviewed.get(key, {})
        if any(package.get(k) != expected.get(k) for k in ("name", "version", "source", "checksum")):
            error(ROOT / "Cargo.lock", f"lockfile contains external package {package['name']!r} without exact reviewed source/checksum")
    if actual != set(reviewed):
        error(review_path, "reviewed transitive package graph differs from Cargo.lock")


def check_font_resources() -> None:
    """Font and language resources stay pinned and retain redistributable notices."""
    fonts = ROOT / "crates/rustic-text/tests/fonts/shaping"
    try:
        manifest = json.loads((fonts / "manifest.json").read_text(encoding="utf-8"))
        for name, digest in {**manifest["outputs"], **manifest["notices"]}.items():
            if hashlib.sha256((fonts / name).read_bytes()).hexdigest() != digest:
                error(fonts / name, "font fixture digest differs from manifest")
        if not (fonts / "OFL-LICENSE.txt").is_file() or not (fonts / "README.md").is_file():
            error(fonts, "font fixture licenses are missing")
        hyphen = ROOT / "crates/rustic-text/data/hyphenation"
        manifest = json.loads((hyphen / "manifest.json").read_text(encoding="utf-8"))
        if hashlib.sha256((hyphen / "en-us.tex").read_bytes()).hexdigest() != manifest["sha256"]:
            error(hyphen, "hyphenation source digest differs from manifest")
    except (OSError, KeyError, TypeError, ValueError) as exc:
        error(fonts, f"invalid text resource metadata: {exc}")


def check_debt_markers(files: list[Path]) -> None:
    """Debt markers must cite a TD-NN entry that exists in docs/TECH_DEBT.md."""
    ledger = ROOT / "docs/TECH_DEBT.md"
    known = set(re.findall(r"^### (TD-\d+)\b", ledger.read_text(encoding="utf-8"), re.MULTILINE))
    for path in files:
        relative = path.relative_to(ROOT)
        if (path.suffix not in {".rs", ".py", ".yml", ".yaml", ".toml", ".sh"}
                or "fixtures" in relative.parts
                or path.name in {"check_repository.py", "test_check_repository.py"}):
            continue
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if DEBT_MARKER.search(line) and not DEBT_ID.search(line):
                error(path, f"line {number}: debt marker must cite a TD-NN entry from docs/TECH_DEBT.md")
            for debt_id in DEBT_ID.findall(line):
                if debt_id not in known:
                    error(path, f"line {number}: {debt_id} is not recorded in docs/TECH_DEBT.md")


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

    idna_manifest = ROOT / "crates/rustic-url/tests/fixtures/idna_manifest.json"
    try:
        manifest = json.loads(idna_manifest.read_text(encoding="utf-8"))
        for output, digest in manifest["outputs"].items():
            if hashlib.sha256((ROOT / output).read_bytes()).hexdigest() != digest:
                error(ROOT / output, "generated IDNA output differs from its manifest; rerun tools/generate_idna_data.py")
        if not (ROOT / "crates/rustic-url/UNICODE-LICENSE.md").is_file():
            error(idna_manifest, "missing Unicode license for generated IDNA data")
        for key in ("unicodetools_revision", "wpt_revision"):
            if not re.fullmatch(r"[0-9a-f]{40}", manifest[key]):
                error(idna_manifest, f"{key} must be a full commit SHA")
    except (OSError, KeyError, TypeError, json.JSONDecodeError) as exc:
        error(idna_manifest, f"invalid IDNA data manifest: {exc}")

    text_manifest = ROOT / "crates/rustic-text/tests/fixtures/text_data_manifest.json"
    try:
        manifest = json.loads(text_manifest.read_text(encoding="utf-8"))
        for output, digest in manifest["outputs"].items():
            if hashlib.sha256((ROOT / output).read_bytes()).hexdigest() != digest:
                error(ROOT / output, "generated text data differs from its manifest; rerun tools/generate_text_data.py")
        if not (ROOT / "crates/rustic-text/UNICODE-LICENSE.md").is_file():
            error(text_manifest, "missing Unicode license for generated text data")
        if not re.fullmatch(r"[0-9a-f]{40}", manifest["unicodetools_revision"]):
            error(text_manifest, "unicodetools_revision must be a full commit SHA")
    except (OSError, KeyError, TypeError, json.JSONDecodeError) as exc:
        error(text_manifest, f"invalid text data manifest: {exc}")

    check_debt_markers(files)
    check_html_fixture("html5lib")
    check_html_fixture("references")
    check_html_fixture("text")
    check_entities()
    try:
        import_css_tests.check()
    except (OSError, ValueError) as exc:
        error(import_css_tests.MANIFEST, str(exc))

    try:
        import_wpt_css_tests.check()
    except (OSError, ValueError, KeyError) as exc:
        error(import_wpt_css_tests.MANIFEST, str(exc))

    try:
        import_wpt_flexbox_tests.check()
    except (OSError, ValueError, KeyError) as exc:
        error(import_wpt_flexbox_tests.MANIFEST, str(exc))

    try:
        import_html_tree_tests.check()
    except (OSError, ValueError) as exc:
        error(ROOT / "crates/rustic-html/tests/fixtures/tree/manifest.json", str(exc))

    try:
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
        toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]
        for member in workspace["members"]:
            if not (ROOT / member / "Cargo.toml").is_file():
                error(ROOT / "Cargo.toml", f"workspace member missing: {member}")
        workflow = ROOT / ".github/workflows/ci.yml"
        workflow_source = workflow.read_text()
        msrv = workspace["package"]["rust-version"]
        if f'"{msrv}.0"' not in workflow_source:
            error(workflow, "compatibility matrix must include workspace MSRV")
        check_workflows(toolchain["channel"])
        check_dependencies(workspace["members"])
    except (OSError, KeyError, tomllib.TOMLDecodeError) as exc:
        ERRORS.append(f"repository configuration: {exc}")

    if ERRORS:
        print("\n".join(ERRORS), file=sys.stderr)
        return 1
    print(f"Repository checks passed ({len(files)} source files inspected).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
