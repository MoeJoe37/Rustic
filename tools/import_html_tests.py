#!/usr/bin/env python3
"""Generate fixed declaration/reference tokenizer corpora from pinned html5lib data."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
DESTINATION = ROOT / "crates/rustic-html/tests/fixtures"
FILES = tuple(f"tokenizer/{name}.test" for name in ("test1", "test2", "test3", "test4", "domjs"))
REFERENCE_FILES = tuple(f"tokenizer/{name}.test" for name in ("entities", "namedEntities", "numericEntities"))
POLICY = "html-declarations-data-state-no-double-escapes-surrogates-ampersands-v1"
REFERENCE_POLICY = "html-references-data-state-no-double-escapes-surrogates-unsupported-errors-v1"
ERRORS = {
    "unexpected-null-character": "UnexpectedNull",
    "invalid-first-character-of-tag-name": "InvalidTagStart",
    "missing-end-tag-name": "MissingEndTagName",
    "eof-before-tag-name": "EofBeforeTagName",
    "incorrectly-opened-comment": "IncorrectlyOpenedComment",
    "abrupt-closing-of-empty-comment": "AbruptClosingOfEmptyComment",
    "nested-comment": "NestedComment",
    "incorrectly-closed-comment": "IncorrectlyClosedComment",
    "eof-in-comment": "EofInComment",
    "cdata-in-html-content": "CdataInHtmlContent",
    "missing-whitespace-before-doctype-name": "MissingWhitespaceBeforeDoctypeName",
    "missing-doctype-name": "MissingDoctypeName",
    "eof-in-doctype": "EofInDoctype",
    "invalid-character-sequence-after-doctype-name": "InvalidSequenceAfterDoctypeName",
    "missing-whitespace-after-doctype-public-keyword": "MissingWhitespaceAfterDoctypePublicKeyword",
    "missing-doctype-public-identifier": "MissingDoctypePublicIdentifier",
    "missing-quote-before-doctype-public-identifier": "MissingQuoteBeforeDoctypePublicIdentifier",
    "abrupt-doctype-public-identifier": "AbruptDoctypePublicIdentifier",
    "missing-whitespace-between-doctype-public-and-system-identifiers": "MissingWhitespaceBetweenDoctypeIdentifiers",
    "missing-quote-before-doctype-system-identifier": "MissingQuoteBeforeDoctypeSystemIdentifier",
    "missing-whitespace-after-doctype-system-keyword": "MissingWhitespaceAfterDoctypeSystemKeyword",
    "missing-doctype-system-identifier": "MissingDoctypeSystemIdentifier",
    "abrupt-doctype-system-identifier": "AbruptDoctypeSystemIdentifier",
    "unexpected-character-after-doctype-system-identifier": "UnexpectedCharacterAfterDoctypeSystemIdentifier",
    "invalid-first-character-of-processing-instruction-target": "InvalidFirstInstructionTarget",
    "invalid-processing-instruction-target": "InvalidInstructionTarget",
    "disallowed-processing-instruction-target": "DisallowedInstructionTarget",
    "eof-in-processing-instruction": "EofInProcessingInstruction",
    "missing-semicolon-after-character-reference": "MissingSemicolonAfterCharacterReference",
    "unknown-named-character-reference": "UnknownNamedCharacterReference",
    "absence-of-digits-in-numeric-character-reference": "AbsenceOfDigitsInNumericReference",
    "null-character-reference": "NullCharacterReference",
    "character-reference-outside-unicode-range": "CharacterReferenceOutsideUnicodeRange",
    "surrogate-character-reference": "SurrogateCharacterReference",
    "noncharacter-character-reference": "NoncharacterCharacterReference",
    "control-character-reference": "ControlCharacterReference",
    "unexpected-character-in-unquoted-attribute-value": "UnexpectedAttributeCharacter",
}


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)
    elif isinstance(value, dict):
        for key, item in value.items():
            yield key
            yield from strings(item)


def context_exclusion(case: dict) -> str | None:
    if case.get("initialStates", ["Data state"]) != ["Data state"] or "lastStartTag" in case:
        return "non_default_state_or_context"
    if case.get("doubleEscaped"):
        return "double_escaped"
    if any(0xD800 <= ord(char) <= 0xDFFF for value in strings([case["input"], case["output"]]) for char in value):
        return "surrogate_input_or_output"
    return None


def exclusion(case: dict) -> str | None:
    if reason := context_exclusion(case):
        return reason
    if not case["input"].startswith(("<!", "<?", "</")):
        return "outside_declaration_entry_points"
    if "&" in case["input"]:
        return "conservative_ampersand_exclusion"
    if any(token[0] not in {"Comment", "DOCTYPE", "ProcessingInstruction", "Character"} for token in case["output"]):
        return "outside_declaration_token_types"
    if any(error["code"] not in ERRORS for error in case.get("errors", [])):
        return "outside_declared_error_codes"
    return None


def reference_exclusion(case: dict) -> str | None:
    if reason := context_exclusion(case):
        return reason
    if any(token[0] not in {"Comment", "DOCTYPE", "ProcessingInstruction", "Character", "StartTag"} for token in case["output"]):
        return "outside_implemented_token_types"
    if any(error["code"] not in ERRORS for error in case.get("errors", [])):
        return "outside_declared_error_codes"
    return None


def literal(value: str) -> str:
    # ASCII-only Rust literals keep controls, line separators and non-ASCII data
    # explicit. Surrogates are excluded before emitting a Rust scalar literal.
    return '"' + "".join(
        "\\" + char if char in {'"', "\\"} else char if 32 <= ord(char) < 127 else f"\\u{{{ord(char):x}}}"
        for char in value
    ) + '"'


def optional(value: str | None) -> str:
    return "None" if value is None else f"Some({literal(value)})"


def expected(token: list) -> str:
    kind = token[0]
    if kind in {"Comment", "Character"}:
        return f"Expected::{kind}({literal(token[1])})"
    if kind == "ProcessingInstruction":
        return f"Expected::Instruction({literal(token[1])}, {literal(token[2])})"
    if kind == "DOCTYPE":
        return f"Expected::Doctype({optional(token[1])}, {optional(token[2])}, {optional(token[3])}, {str(not token[4]).lower()})"
    if kind == "StartTag":
        attributes = ", ".join(f"({literal(name)}, {literal(value)})" for name, value in token[2].items())
        flag = len(token) > 3 and token[3]
        return f"Expected::StartTag({literal(token[1])}, &[{attributes}], {str(bool(flag)).lower()})"
    raise ValueError(f"unselected token kind: {kind}")


def generate(sources: dict[str, bytes], revision: str, suite: str = "declarations") -> tuple[str, str]:
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("revision must be the full upstream commit SHA")
    if suite not in {"declarations", "references"}:
        raise ValueError("unknown tokenizer suite")
    files = FILES if suite == "declarations" else REFERENCE_FILES
    select = exclusion if suite == "declarations" else reference_exclusion
    lines = [
        "// Generated by tools/import_html_tests.py. Do not edit by hand.",
        f"// html5lib/html5lib-tests revision: {revision}; MIT, see HTML5LIB-LICENSE.md.",
        "use super::{Case, Expected};", "use rustic_html::DiagnosticKind as D;", "",
        "#[rustfmt::skip]", "pub const CASES: &[Case] = &[",
    ]
    manifests = []
    total = selected = 0
    token_counts: Counter = Counter()
    for path in files:
        source = sources[path]
        cases = json.loads(source)["tests"]
        indices = []
        reasons: Counter = Counter()
        total += len(cases)
        for index, case in enumerate(cases):
            reason = select(case)
            if reason:
                reasons[reason] += 1
                continue
            selected += 1
            indices.append(index)
            token_counts.update(token[0] for token in case["output"])
            output = ", ".join(expected(token) for token in case["output"])
            errors = ", ".join(f"D::{ERRORS[error['code']]}" for error in case.get("errors", []))
            lines.append(f"    Case {{ file: {literal(path)}, index: {index}, description: {literal(case['description'])}, input: {literal(case['input'])}, output: &[{output}], errors: &[{errors}] }},")
        manifests.append({
            "path": path,
            "source_url": f"https://github.com/html5lib/html5lib-tests/blob/{revision}/{path}",
            "source_git_blob": hashlib.sha1(b"blob " + str(len(source)).encode() + b"\0" + source).hexdigest(),
            "source_sha256": hashlib.sha256(source).hexdigest(),
            "upstream_cases": len(cases), "selected_cases": len(indices),
            "excluded_cases": len(cases) - len(indices), "exclusion_reasons": dict(sorted(reasons.items())),
            "selected_array_indices": indices,
        })
    lines.extend([
        "];", "", f"pub const SELECTED: usize = {selected};",
        f"pub const UPSTREAM: usize = {total};", "",
    ])
    rust = "\n".join(lines)
    manifest = {
        "upstream_repository": "html5lib/html5lib-tests", "upstream_revision": revision,
        "license": "MIT; HTML5LIB-LICENSE.md", "selection_policy": POLICY if suite == "declarations" else REFERENCE_POLICY,
        "fixture_sha256": hashlib.sha256(rust.encode()).hexdigest(),
        "upstream_cases": total, "selected_cases": selected, "excluded_cases": total - selected,
        "expected_token_counts": dict(sorted(token_counts.items())), "sources": manifests,
        "checked_fields": ["complete token stream (adjacent character tokens coalesced)", "doctype optional fields and force-quirks", "ordered parse-error codes", "all two-chunk byte splits and one-byte feeds"],
        "not_checked": ["upstream error line/column positions", "excluded cases/states", "tree construction", "full tokenizer conformance"],
    }
    return rust, json.dumps(manifest, indent=2, sort_keys=True) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-dir", required=True, type=Path, help="pinned html5lib-tests checkout root")
    parser.add_argument("--revision", required=True)
    parser.add_argument("--suite", choices=["declarations", "references"], default="declarations")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    try:
        files = FILES if args.suite == "declarations" else REFERENCE_FILES
        sources = {path: (args.source_dir / path).read_bytes() for path in files}
        rust, manifest = generate(sources, args.revision, args.suite)
        stem = "html5lib" if args.suite == "declarations" else "references"
        outputs = {DESTINATION / f"{stem}_cases.rs": rust, DESTINATION / f"{stem}_manifest.json": manifest}
        if args.check:
            stale = [str(path.relative_to(ROOT)) for path, value in outputs.items() if not path.exists() or path.read_text(encoding="utf-8") != value]
            if stale:
                raise ValueError("stale HTML fixture outputs: " + ", ".join(stale))
        else:
            DESTINATION.mkdir(parents=True, exist_ok=True)
            for path, value in outputs.items():
                path.write_text(value, encoding="utf-8", newline="\n")
        summary = json.loads(manifest)
        print(f"HTML {args.suite} subset: {summary['selected_cases']} selected / {summary['upstream_cases']} upstream; {summary['excluded_cases']} excluded.")
        return 0
    except (OSError, ValueError, KeyError, TypeError) as exc:
        print(f"HTML fixture error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
