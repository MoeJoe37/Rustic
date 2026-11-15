#!/usr/bin/env python3
"""Generate Intl.PluralRules data from the pinned CLDR 48 archive."""

import argparse
import re
import subprocess
from pathlib import Path
from xml.etree import ElementTree
from zipfile import ZipFile

from generate_js_collation import CORE_SHA, checked

DESTINATION = (Path(__file__).resolve().parents[1] /
               "crates/rustic-js/src/runtime/builtins/intl/plural_rules/data.rs")
CATEGORY = ["zero", "one", "two", "few", "many", "other"]
OPERAND = {"n": "Number", "i": "Integer", "v": "Visible", "f": "Fraction",
           "t": "TrimmedFraction", "e": "Exponent"}
RELATION = re.compile(r"([nivfte])(?: % (\d+))? (!?=) (\d+(?:\.\.\d+)?(?:,\d+(?:\.\.\d+)?)*)")


def archive_xml(archive, name):
    return ElementTree.fromstring(archive.read(f"common/supplemental/{name}.xml"))


def locale_table(output, name, locales):
    output.append(f"pub(super) const {name}: &[(&str, usize)] = &[")
    for locale, index in sorted(locales.items()):
        output.append(f'    ("{locale}", {index}),')
    output.append("];")


def condition(expression):
    relations = []
    for text in expression.split(" and "):
        relation = RELATION.fullmatch(text)
        if relation is None:
            raise ValueError(f"unrecognized plural relation {text!r}")
        operand, modulo, operator, values = relation.groups()
        pairs = []
        for value in values.split(","):
            start, _, end = value.partition("..")
            pairs.append(f"({start}, {end or start})")
        ranges = ", ".join(pairs)
        negation = "!" if operator == "!=" else ""
        relations.append(f"{negation}n.matches({OPERAND[operand]}, {modulo or 0}, &[{ranges}])")
    return " && ".join(relations)


def rules(archive, output, filename, name):
    root = archive_xml(archive, filename)
    locales = {}
    tests = []
    for index, group in enumerate(root.findall(".//pluralRules")):
        for locale in group.attrib["locales"].split():
            locales[locale.replace("_", "-")] = index
        cases = {CATEGORY.index(rule.attrib["count"]): rule.text.partition("@")[0].strip()
                 for rule in group.findall("pluralRule")}
        expression = str(CATEGORY.index("other"))
        for category, rule in reversed(sorted(cases.items())):
            if category == CATEGORY.index("other") and not rule:
                continue
            if not rule:
                raise ValueError(f"missing expression for {CATEGORY[category]}")
            match = " || ".join(condition(clause) for clause in rule.split(" or "))
            expression = f"if {match} {{ {category} }} else {{ {expression} }}"
        mask = sum(1 << category for category in cases)
        closure = "|_|" if expression == "5" else "|n|"
        tests.append(f"    ({mask}, {closure} {expression}),")
    locale_table(output, f"{name}_LOCALES", locales)
    output.append(f"pub(super) const {name}_RULES: &[Rule] = &[")
    output.extend(tests)
    output.append("];")


def ranges(archive, output):
    root = archive_xml(archive, "pluralRanges")
    locales = {}
    groups = []
    for index, group in enumerate(root.findall(".//pluralRanges")):
        for locale in group.attrib["locales"].split():
            locales[locale.replace("_", "-")] = index
        values = [CATEGORY.index("other")] * 36
        for rule in group.findall("pluralRange"):
            start = CATEGORY.index(rule.attrib["start"])
            end = CATEGORY.index(rule.attrib["end"])
            values[start * 6 + end] = CATEGORY.index(rule.attrib["result"])
        groups.append(f"    {values},")
    locale_table(output, "RANGE_LOCALES", locales)
    output.append("pub(super) const RANGE_RULES: &[[usize; 36]] = &[")
    output.extend(groups)
    output.append("];")


def explicit_parents(archive):
    supplemental = archive_xml(archive, "supplementalData")
    return {
        locale: rule.attrib["parent"]
        for table in supplemental.findall("parentLocales")
        if "component" not in table.attrib
        for rule in table.findall("parentLocale")
        for locale in rule.attrib["locales"].split()
    }


def compact_scales(archive, output):
    parents = explicit_parents(archive)
    formats = {}
    maximum = 0
    for path in archive.namelist():
        if not path.startswith("common/main/") or not path.endswith(".xml"):
            continue
        locale = Path(path).stem
        root = ElementTree.fromstring(archive.read(path))
        formats[locale] = {}, {}
        for index, style in enumerate(("short", "long")):
            patterns = root.findall(
                ".//decimalFormats[@numberSystem='latn']"
                f"/decimalFormatLength[@type='{style}']/decimalFormat/pattern[@count='other']"
            )
            for pattern in patterns:
                if "alt" in pattern.attrib or pattern.text == "↑↑↑":
                    continue
                magnitude = len(pattern.attrib["type"]) - 1
                maximum = max(maximum, magnitude)
                if pattern.text == "0":
                    scale = 0
                else:
                    digits = re.search(r"0+", pattern.text)
                    if digits is None:
                        # Literal patterns are legal for a single category.
                        # Their scale follows the entry for the same power.
                        continue
                    scale = magnitude - len(digits.group()) + 1
                    if scale < 0:
                        raise ValueError(f"invalid compact power {pattern.attrib}")
                formats[locale][index][magnitude] = scale

    unique = {}
    locales = {}
    for locale in sorted(formats):
        rows = ({}, {})
        parent = locale
        seen = set()
        while parent not in seen:
            seen.add(parent)
            for target, source in zip(rows, formats.get(parent, ({}, {}))):
                for magnitude, scale in source.items():
                    target.setdefault(magnitude, scale)
            if parent == "root":
                break
            parent = parents.get(parent, parent.rpartition("_")[0] or "root")
        key = []
        for index in range(2):
            scales = rows[0] | rows[index]
            last = 0
            powers = []
            for magnitude in range(maximum + 1):
                last = scales.get(magnitude, last)
                powers.append(last)
            key.append(tuple(powers))
        key = tuple(key)
        locales[locale.replace("_", "-")] = unique.setdefault(key, len(unique))
    locale_table(output, "COMPACT_LOCALES", locales)
    output.append(f"pub(super) const COMPACT_SCALES: &[[[u8; {maximum + 1}]; 2]] = &[")
    for short, long in unique:
        output.append(f"    {[list(short), list(long)]},")
    output.append("];")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("core", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    output = ["//! Generated by tools/generate_js_plural_rules.py from CLDR 48.",
              "", "use super::{Operand::*, Rule};"]
    with ZipFile(checked(args.core, CORE_SHA)) as archive:
        rules(archive, output, "plurals", "CARDINAL")
        rules(archive, output, "ordinals", "ORDINAL")
        ranges(archive, output)
        compact_scales(archive, output)
    source = subprocess.run(["rustfmt", "--emit", "stdout", "--edition", "2024"],
                            input="\n".join(output) + "\n", text=True,
                            check=True, capture_output=True).stdout
    if args.check:
        if source != DESTINATION.read_text(encoding="utf-8"):
            raise SystemExit("generated plural rules differ")
    else:
        DESTINATION.parent.mkdir(exist_ok=True)
        DESTINATION.write_text(source, encoding="utf-8")


if __name__ == "__main__":
    main()
