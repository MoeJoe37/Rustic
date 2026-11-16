#!/usr/bin/env python3
"""Generate Rust binding metadata from the pinned DOM text interfaces."""

import argparse
from dataclasses import dataclass
from enum import Enum
import hashlib
import json
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
INPUTS = ROOT / "crates/rustic-bindings/idl"
MANIFEST = INPUTS / "dom_manifest.json"
SUBSET = INPUTS / "dom-text.idl"
LICENSE = INPUTS / "WHATWG-DOM-LICENSE.txt"
OUTPUT = ROOT / "crates/rustic-bindings/src/metadata/generated.rs"
IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z_0-9]*")
TOKEN = re.compile(r'\s+|[A-Za-z_][A-Za-z_0-9]*|""|[{}:;\[\]=(),]')


class ValueType(Enum):
    DOM_STRING = "ValueType::DomString(DomStringConversion::Default)"
    LEGACY_DOM_STRING = "ValueType::DomString(DomStringConversion::LegacyNullToEmptyString)"
    UNSIGNED_LONG = "ValueType::UnsignedLong"


@dataclass(frozen=True)
class Argument:
    name: str
    value_type: ValueType
    default_empty_string: bool


@dataclass(frozen=True)
class Attribute:
    name: str
    value_type: ValueType
    readonly: bool


@dataclass(frozen=True)
class Constructor:
    arguments: tuple[Argument, ...]


@dataclass(frozen=True)
class Operation:
    name: str
    returns: ValueType | str | None
    arguments: tuple[Argument, ...]
    new_object: bool


@dataclass(frozen=True)
class Interface:
    name: str
    parent: str | None
    members: tuple[Attribute | Constructor | Operation, ...]


class Parser:
    def __init__(self, text: str):
        self.tokens: list[tuple[str, int]] = []
        index = 0
        while index < len(text):
            match = TOKEN.match(text, index)
            if not match:
                line = text.count("\n", 0, index) + 1
                raise ValueError(f"line {line}: unsupported IDL token {text[index]!r}")
            token = match[0]
            if not token.isspace():
                self.tokens.append((token, index))
            index = match.end()
        self.tokens.append(("", len(text)))
        self.text = text
        self.index = 0

    def peek(self) -> str:
        return self.tokens[self.index][0]

    def error(self, detail: str) -> ValueError:
        line = self.text.count("\n", 0, self.tokens[self.index][1]) + 1
        return ValueError(f"line {line}: {detail}; found {self.peek()!r}")

    def accept(self, expected: str) -> bool:
        if self.peek() != expected:
            return False
        self.index += 1
        return True

    def expect(self, expected: str) -> None:
        if not self.accept(expected):
            raise self.error(f"expected {expected!r}")

    def name(self) -> str:
        token = self.peek()
        if not IDENTIFIER.fullmatch(token):
            raise self.error("expected an identifier")
        self.index += 1
        return token

    def value_type(self) -> ValueType:
        if self.accept("DOMString"):
            return ValueType.DOM_STRING
        if self.accept("unsigned"):
            self.expect("long")
            return ValueType.UNSIGNED_LONG
        raise self.error("unsupported value type")

    def arguments(self) -> tuple[Argument, ...]:
        self.expect("(")
        if self.accept(")"):
            return ()
        arguments: list[Argument] = []
        names: set[str] = set()
        optional_seen = False
        while True:
            optional = self.accept("optional")
            if optional_seen and not optional:
                raise self.error("required argument follows an optional one")
            value_type = self.value_type()
            name = self.name()
            if name in names:
                raise self.error(f"duplicate argument {name!r}")
            names.add(name)
            if optional:
                if value_type != ValueType.DOM_STRING:
                    raise self.error("only optional DOMString with an empty default is supported")
                self.expect("=")
                self.expect('""')
                optional_seen = True
            arguments.append(Argument(name, value_type, optional))
            if not self.accept(","):
                self.expect(")")
                return tuple(arguments)

    def member(self, interfaces: set[str]) -> Attribute | Constructor | Operation:
        if self.accept("constructor"):
            member = Constructor(self.arguments())
        elif self.peek() in ("readonly", "attribute"):
            readonly = self.accept("readonly")
            self.expect("attribute")
            legacy = self.accept("[")
            if legacy:
                self.expect("LegacyNullToEmptyString")
                self.expect("]")
            value_type = self.value_type()
            if legacy:
                if readonly or value_type != ValueType.DOM_STRING:
                    raise self.error("LegacyNullToEmptyString needs a writable DOMString attribute")
                value_type = ValueType.LEGACY_DOM_STRING
            member = Attribute(self.name(), value_type, readonly)
        else:
            new_object = self.accept("[")
            if new_object:
                self.expect("NewObject")
                self.expect("]")
            returns: ValueType | str | None
            if self.accept("undefined"):
                returns = None
            elif self.peek() in ("DOMString", "unsigned"):
                returns = self.value_type()
            elif self.peek() in interfaces:
                returns = self.name()
            else:
                raise self.error("unsupported operation return type")
            if new_object and not isinstance(returns, str):
                raise self.error("NewObject needs an interface return type")
            member = Operation(self.name(), returns, self.arguments(), new_object)
        self.expect(";")
        return member

    def interface(self, interfaces: set[str]) -> Interface:
        for token in ("[", "Exposed", "=", "Window", "]", "interface"):
            self.expect(token)
        name = self.name()
        parent = self.name() if self.accept(":") else None
        if parent is not None and parent not in interfaces | {"Node"}:
            raise self.error(f"unsupported parent {parent!r}")
        self.expect("{")
        members: list[Attribute | Constructor | Operation] = []
        names: set[str] = set()
        while not self.accept("}"):
            member = self.member(interfaces)
            member_name = "constructor" if isinstance(member, Constructor) else member.name
            if member_name in names:
                raise self.error(f"duplicate or overloaded member {member_name!r}")
            names.add(member_name)
            members.append(member)
        self.expect(";")
        return Interface(name, parent, tuple(members))


def parse_subset(text: str, names: list[str]) -> list[Interface]:
    parser = Parser(text)
    names_set = set(names)
    interfaces = [parser.interface(names_set) for _ in names]
    parser.expect("")
    if [interface.name for interface in interfaces] != names:
        raise ValueError("IDL declarations differ from the manifest interface list or order")
    parents = {interface.name: interface.parent for interface in interfaces}
    for name in names:
        seen = {name}
        parent = parents[name]
        while parent in parents:
            if parent in seen:
                raise ValueError(f"IDL inheritance cycle involving {parent}")
            seen.add(parent)
            parent = parents[parent]
    return interfaces


def extract_subset(source: str, names: list[str]) -> bytes:
    blocks = re.findall(r"<pre class=idl>(.*?)</pre>", source, re.S)
    selected = []
    for name in names:
        pattern = re.compile(rf"^interface {re.escape(name)}\b", re.M)
        matches = [block.strip() for block in blocks if pattern.search(block)]
        if len(matches) != 1:
            raise ValueError(f"expected one upstream IDL block for {name}, found {len(matches)}")
        selected.append(matches[0])
    return ("\n\n".join(selected) + "\n").encode("utf-8")


def rust_bool(value: bool) -> str:
    return "true" if value else "false"


def emit_arguments(arguments: tuple[Argument, ...]) -> list[str]:
    lines = []
    for argument in arguments:
        default = "Some(ArgumentDefault::EmptyString)" if argument.default_empty_string else "None"
        lines.extend([
            "                    Argument {",
            f'                        name: "{argument.name}",',
            f"                        value_type: {argument.value_type.value},",
            f"                        default: {default},",
            "                    },",
        ])
    return lines


def render(interfaces: list[Interface], revision: str, digest: str) -> bytes:
    lines = [
        "// Generated by tools/generate_dom_idl.py.",
        f"// WHATWG DOM revision {revision}.",
        f"// IDL subset SHA-256 {digest}.",
        "// Upstream notice: crates/rustic-bindings/idl/WHATWG-DOM-LICENSE.txt.",
        "",
        "use super::{Argument, ArgumentDefault, Interface, Member, ReturnType, ValueType};",
        "use crate::webidl::DomStringConversion;",
        "",
        "#[rustfmt::skip]",
        "pub(super) const INTERFACES: &[Interface] = &[",
    ]
    for interface in interfaces:
        parent = f'Some("{interface.parent}")' if interface.parent is not None else "None"
        lines.extend([
            "    Interface {",
            f'        name: "{interface.name}",',
            f"        parent: {parent},",
            "        members: &[",
        ])
        for member in interface.members:
            if isinstance(member, Attribute):
                lines.extend([
                    "            Member::Attribute {",
                    f'                name: "{member.name}",',
                    f"                value_type: {member.value_type.value},",
                    f"                readonly: {rust_bool(member.readonly)},",
                    "            },",
                ])
            elif isinstance(member, Constructor):
                lines.extend([
                    "            Member::Constructor(&[",
                    *emit_arguments(member.arguments),
                    "            ]),",
                ])
            else:
                if member.returns is None:
                    returns = "ReturnType::Undefined"
                elif isinstance(member.returns, ValueType):
                    returns = f"ReturnType::Value({member.returns.value})"
                else:
                    returns = f'ReturnType::Interface("{member.returns}")'
                lines.extend([
                    "            Member::Operation {",
                    f'                name: "{member.name}",',
                    f"                returns: {returns},",
                    f"                new_object: {rust_bool(member.new_object)},",
                    "                arguments: &[",
                    *emit_arguments(member.arguments),
                    "                ],",
                    "            },",
                ])
        lines.extend(["        ],", "    },"])
    return ("\n".join(lines + ["];"]) + "\n").encode("utf-8")


def read_verified(path: Path, digest: str) -> bytes:
    contents = path.read_bytes()
    if hashlib.sha256(contents).hexdigest() != digest:
        raise ValueError(f"{path.name}: SHA-256 differs from the pinned manifest")
    return contents


def generate(upstream: Path | None = None) -> bytes:
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    revision = manifest["revision"]
    if not isinstance(revision, str) or not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("DOM revision must be a full Git commit SHA")
    for field in ("dom_bs_sha256", "subset_sha256", "license_sha256"):
        digest = manifest[field]
        if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise ValueError(f"{field} must be a full SHA-256 digest")
    names = manifest["interfaces"]
    if (not isinstance(names, list) or not names
            or any(not isinstance(name, str) or not IDENTIFIER.fullmatch(name) for name in names)
            or len(set(names)) != len(names)):
        raise ValueError("manifest must list distinct interface names")
    subset = read_verified(SUBSET, manifest["subset_sha256"])
    read_verified(LICENSE, manifest["license_sha256"])
    if upstream is not None:
        source = read_verified(upstream, manifest["dom_bs_sha256"])
        if extract_subset(source.decode("utf-8"), names) != subset:
            raise ValueError("the vendored IDL differs from the pinned upstream declarations")
    interfaces = parse_subset(subset.decode("utf-8"), names)
    return render(interfaces, revision, manifest["subset_sha256"])


def check(upstream: Path | None = None) -> None:
    if generate(upstream) != OUTPUT.read_bytes():
        raise ValueError("generated DOM binding metadata is stale; run tools/generate_dom_idl.py")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="verify the committed Rust output")
    parser.add_argument("--upstream-source", type=Path, help="also verify a copy of the pinned dom.bs")
    args = parser.parse_args()
    try:
        if args.check:
            check(args.upstream_source)
        else:
            OUTPUT.write_bytes(generate(args.upstream_source))
    except (OSError, ValueError, KeyError, TypeError) as exc:
        print(f"DOM IDL: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
