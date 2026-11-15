#!/usr/bin/env python3
"""Generate the data packs of Rustic's Intl services from CLDR 48.

Takes the same pinned source directory as tools/generate_js_locale_data.py
and writes crates/rustic-js/data/intl/*.bin with their digests in
crates/rustic-js/tests/fixtures/js_intl_manifest.json. `--check` compares
instead of writing. The pack layout is described in tools/cldr_data.py and
read by crates/rustic-js/src/runtime/builtins/intl/pack.rs.
"""

import argparse
import hashlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from cldr_data import NONE, STEP, Cldr, Pack, path  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "crates/rustic-js/data/intl"
MANIFEST = ROOT / "crates/rustic-js/tests/fixtures/js_intl_manifest.json"
CORE_ZIP = "06c7c698d6fd8d67cefac15a0206b0109b00e0ef1636f86f84449fa959561f74"

COUNTS = ["zero", "one", "two", "few", "many", "other"]
# Field numbers shared with number_data.rs.
SYMBOLS = [
    "decimal", "group", "percentSign", "plusSign", "minusSign", "approximatelySign",
    "exponential", "superscriptingExponent", "perMille", "infinity", "nan",
    "currencyDecimal", "currencyGroup", "timeSeparator",
]
# ECMA-402 table 2: the sanctioned single units, with their CLDR categories.
UNITS = {
    "acre": "area", "bit": "digital", "byte": "digital", "celsius": "temperature",
    "centimeter": "length", "day": "duration", "degree": "angle", "fahrenheit": "temperature",
    "fluid-ounce": "volume", "foot": "length", "gallon": "volume", "gigabit": "digital",
    "gigabyte": "digital", "gram": "mass", "hectare": "area", "hour": "duration",
    "inch": "length", "kilobit": "digital", "kilobyte": "digital", "kilogram": "mass",
    "kilometer": "length", "liter": "volume", "megabit": "digital", "megabyte": "digital",
    "meter": "length", "microsecond": "duration", "mile": "length",
    "mile-scandinavian": "length", "milliliter": "volume", "millimeter": "length",
    "millisecond": "duration", "minute": "duration", "month": "duration",
    "nanosecond": "duration", "ounce": "mass", "percent": "concentr", "petabyte": "digital",
    "pound": "mass", "second": "duration", "stone": "mass", "terabit": "digital",
    "terabyte": "digital", "week": "duration", "yard": "length", "year": "duration",
}
LENGTHS = ["long", "short", "narrow"]


def canonical(locale):
    """CLDR file names as canonical tags: variants are lower case."""
    return "-".join(
        part.lower() if index and len(part) > 4 else part
        for index, part in enumerate(locale.split("-"))
    )


class Locales:
    """Row numbers of the locales, and rows that differ from the parent."""

    def __init__(self, cldr, pack):
        self.cldr = cldr
        self.names = sorted(["root", *cldr.locales], key=canonical)
        self.names.sort(key=lambda name: canonical(name).encode())
        self.row = {name: index for index, name in enumerate(self.names)}
        pack.table("locales", 2, [
            [pack.s(canonical(name)), self.row.get(cldr.parent(name), NONE)]
            for name in self.names
        ])

    def sparse(self, resolve, fallback=lambda key: None):
        """`(row, key, value)` rows from which a lookup recovers every value.

        `resolve(locale)` gives the complete `{key: value}` of a locale.
        The reader looks a key up in the locale and then its parents, and
        when the whole chain lacks it tries `fallback(key)` the same way
        (pack.rs `Table::inherited`). A row is written only where that
        lookup would otherwise give a different answer.
        """
        stored = {}

        def lookup(name, key):
            while key is not None:
                member = name
                while member is not None:
                    if key in stored[member]:
                        return stored[member][key]
                    member = self.cldr.parent(member)
                key = fallback(key)
            return None

        def depth(key):
            steps = 0
            while (key := fallback(key)) is not None:
                steps += 1
            return steps

        out = []
        for name in sorted(self.names, key=lambda name: len(self.cldr.chain(name))):
            stored[name] = {}
            full = resolve(name)
            for key in sorted(full, key=lambda key: (depth(key), key)):
                if lookup(name, key) != full[key]:
                    stored[name][key] = full[key]
                    out.append((self.row[name], key, full[key]))
        return out


def number_pack(cldr):
    pack = Pack()
    locales = Locales(cldr, pack)

    systems = {}
    for entry in cldr.supplemental("numberingSystems").iter("numberingSystem"):
        if entry.attrib["type"] == "numeric":
            systems[entry.attrib["id"]] = entry.attrib["digits"]
    names = sorted(systems)
    system_index = {name: index for index, name in enumerate(names)}
    pack.table("nsys", 2, [[pack.s(name), pack.s(systems[name])] for name in names])

    def numbers(locale):
        tree = cldr.subtree(locale, path("numbers"))
        out = {}
        default = tree.get(path("defaultNumberingSystem"))
        out[0] = system_index[default]
        native = tree.get(path("otherNumberingSystems", "native"))
        out[1] = system_index.get(native, system_index[default])
        out[2] = int(tree.get(path("minimumGroupingDigits"), "1"))
        return out

    pack.table("numbers", 2, sorted(
        [row << 16 | key, value] for row, key, value in locales.sparse(numbers)
    ))

    def symbols(locale):
        out = {}
        for name, system in system_index.items():
            tree = cldr.subtree(locale, path("numbers", f"symbols[@numberSystem='{name}']"))
            for field, symbol in enumerate(SYMBOLS):
                value = tree.get(path(symbol))
                if value is not None:
                    out[system << 8 | field] = pack.s(value)
        return out

    latn = system_index["latn"]

    def to_latn(key):
        # A numbering system without data of its own uses the Latin data.
        return None if key >> 8 == latn else latn << 8 | key & 0xFF

    pack.table("symbols", 2, sorted(
        [row << 16 | key, value] for row, key, value in locales.sparse(symbols, to_latn)
    ))

    def patterns(locale):
        out = {}
        for name, system in system_index.items():
            ns = f"[@numberSystem='{name}']"
            sources = [
                (0, (f"decimalFormats{ns}", "decimalFormatLength", "decimalFormat", "pattern")),
                (1, (f"percentFormats{ns}", "percentFormatLength", "percentFormat", "pattern")),
                (2, (f"scientificFormats{ns}", "scientificFormatLength", "scientificFormat", "pattern")),
                (9, (f"miscPatterns{ns}", "pattern[@type='range']")),
                (10, (f"currencyFormats{ns}", "currencySpacing", "beforeCurrency", "insertBetween")),
                (11, (f"currencyFormats{ns}", "currencySpacing", "afterCurrency", "insertBetween")),
            ]
            for offset, kind in enumerate(["standard", "accounting"]):
                base = (f"currencyFormats{ns}", "currencyFormatLength", f"currencyFormat[@type='{kind}']")
                sources.append((3 + offset, (*base, "pattern")))
                sources.append((5 + offset, (*base, "pattern[@alt='alphaNextToNumber']")))
            for index, count in enumerate(COUNTS):
                sources.append((16 + index, (f"currencyFormats{ns}", f"unitPattern[@count='{count}']")))
            for kind, steps in sources:
                value = cldr.get(locale, "numbers", *steps)
                if value is not None:
                    out[system << 8 | kind] = pack.s(value)
        return out

    pack.table("patterns", 2, sorted(
        [row << 16 | key, value] for row, key, value in locales.sparse(patterns, to_latn)
    ))

    def compact(locale):
        out = {}
        for name, system in system_index.items():
            ns = f"[@numberSystem='{name}']"
            for kind, steps in enumerate([
                (f"decimalFormats{ns}", "decimalFormatLength[@type='short']", "decimalFormat"),
                (f"decimalFormats{ns}", "decimalFormatLength[@type='long']", "decimalFormat"),
                (f"currencyFormats{ns}", "currencyFormatLength[@type='short']", "currencyFormat[@type='standard']"),
            ]):
                tree = cldr.subtree(locale, path("numbers", *steps))
                for (leaf,), value in ((key, value) for key, value in tree.items() if len(key) == 1):
                    attributes = dict(part.split("=") for part in leaf[len("pattern[@"):-1].split("][@"))
                    if "alt" in attributes or attributes["count"].strip("'") not in COUNTS:
                        continue
                    zeros = len(attributes["type"].strip("'")) - 1
                    count = COUNTS.index(attributes["count"].strip("'"))
                    out[(system << 8 | kind, zeros << 8 | count)] = pack.s(value)
        return out

    def compact_fallback(key):
        # Other numbering systems use the Latin data; long forms default
        # to the short ones.
        first, second = key
        if first >> 8 != latn:
            return (latn << 8 | first & 0xFF, second)
        return (first & ~0xFF, second) if first & 0xFF == 1 else None

    pack.table("compact", 3, sorted(
        [row << 16 | key[0], key[1], value]
        for row, key, value in locales.sparse(compact, compact_fallback)
    ))

    fractions = {}
    for info in cldr.supplemental("supplementalData").iter("info"):
        if "iso4217" in info.attrib:
            fractions[info.attrib["iso4217"]] = int(info.attrib["digits"])
    codes = set(fractions) - {"DEFAULT"}
    for locale in ("root", "en"):
        for (leaf, *_rest) in cldr.subtree(locale, path("numbers", "currencies")):
            codes.add(leaf[len("currency[@type='"):-2])
    codes = sorted(codes)
    code_index = {code: index for index, code in enumerate(codes)}
    pack.table("currency_codes", 2, [
        [pack.s(code), fractions.get(code, fractions["DEFAULT"])] for code in codes
    ])

    def currencies(locale):
        out = {}
        tree = cldr.subtree(locale, path("numbers", "currencies"))
        for (leaf, field), value in ((key, value) for key, value in tree.items() if len(key) == 2):
            code = leaf[len("currency[@type='"):-2]
            if code not in code_index:
                continue
            if field == "symbol":
                kind = 0
            elif field == "symbol[@alt='narrow']":
                kind = 1
            elif field == "displayName":
                kind = 2
            elif field.startswith("displayName[@count='"):
                kind = 3 + COUNTS.index(field[len("displayName[@count='"):-2])
            else:
                continue
            out[code_index[code] << 4 | kind] = pack.s(value)
        return out

    pack.table("currencies", 2, sorted(
        [row << 16 | key, value] for row, key, value in locales.sparse(currencies)
    ))

    # Single units, then the compound units CLDR translates as a whole.
    unit_types = {name: f"{category}-{name}" for name, category in UNITS.items()}
    for (leaf, *_rest) in cldr.subtree("root", path("units", "unitLength[@type='short']")):
        if leaf.startswith("unit[@type='") and "-per-" in leaf:
            cldr_type = leaf[len("unit[@type='"):-2]
            core = cldr_type.split("-", 1)[1]
            numerator, _, denominator = core.partition("-per-")
            if numerator in UNITS and denominator in UNITS:
                unit_types[core] = cldr_type
    unit_names = sorted(unit_types)
    unit_index = {name: index for index, name in enumerate(unit_names)}
    per = len(unit_names)
    pack.table("unit_ids", 1, [[pack.s(name)] for name in unit_names])

    def units(locale):
        out = {}
        for length, width in enumerate(LENGTHS):
            tree = cldr.subtree(locale, path("units", f"unitLength[@type='{width}']"))
            for name, index in unit_index.items():
                base = path(f"unit[@type='{unit_types[name]}']")
                for count, label in enumerate(COUNTS):
                    value = tree.get(base + path(f"unitPattern[@count='{label}']"))
                    if value is not None:
                        out[index << 5 | length << 3 | count] = pack.s(value)
                value = tree.get(base + path("perUnitPattern"))
                if value is not None:
                    out[index << 5 | length << 3 | 6] = pack.s(value)
            value = tree.get(path("compoundUnit[@type='per']", "compoundUnitPattern"))
            if value is not None:
                out[per << 5 | length << 3] = pack.s(value)
        return out

    def to_short(key):
        # Root aliases the long and narrow forms to the short ones.
        return None if key >> 3 & 3 == 1 else key & ~0x18 | 1 << 3

    pack.table("units", 2, sorted(
        [row << 16 | key, value] for row, key, value in locales.sparse(units, to_short)
    ))

    for name, kind in (("plurals", 0), ("ordinals", 1)):
        rows = []
        for rules in cldr.supplemental(name).iter("pluralRules"):
            cells = [pack.s("")] * len(COUNTS)
            for rule in rules.iter("pluralRule"):
                text = rule.text.split("@", 1)[0].strip()
                cells[COUNTS.index(rule.attrib["count"])] = pack.s(text) if text else pack.s("")
            present = sum(
                1 << COUNTS.index(rule.attrib["count"]) for rule in rules.iter("pluralRule")
            )
            for locale in rules.attrib["locales"].split():
                rows.append([pack.s(locale.replace("_", "-")), present, *cells])
        rows.sort(key=lambda row: pack.strings[row[0]].encode())
        pack.table(f"plural_{kind}", 2 + len(COUNTS), rows)

    return pack.bytes()


RELATIVE_UNITS = ["second", "minute", "hour", "day", "week", "month", "quarter", "year"]


def attributes(step):
    """The tag and the attributes of a path step."""
    return step.split("[", 1)[0], dict(STEP.findall(step))


def relative_pack(cldr):
    """Relative time patterns: `relative[locale << 16 | key, text]`.

    `key` is `unit << 8 | width << 6 | kind`, with the unit's index in
    RELATIVE_UNITS, the width's in LENGTHS, and as kind a plural category
    for the future, 8 plus one for the past, or 18 plus the offset of a
    named day such as "yesterday" (-1).
    """
    pack = Pack()
    locales = Locales(cldr, pack)

    def fields(locale):
        result = {}
        for steps, text in cldr.subtree(locale, path("dates", "fields")).items():
            tag, attrs = attributes(steps[0])
            name, _, width = attrs.get("type", "").partition("-")
            if tag != "field" or name not in RELATIVE_UNITS:
                continue
            key = RELATIVE_UNITS.index(name) << 8 | LENGTHS.index(width or "long") << 6
            tag, attrs = attributes(steps[1])
            if tag == "relative" and len(steps) == 2 and -2 <= int(attrs["type"]) <= 3:
                result[key | 18 + int(attrs["type"])] = pack.s(text)
            elif tag == "relativeTime" and len(steps) == 3:
                count = attributes(steps[2])[1]["count"]
                past = 8 if attrs["type"] == "past" else 0
                result[key | past | COUNTS.index(count)] = pack.s(text)
        return result

    def shorter(key):
        # Root aliases narrow to short and short to long.
        width = key >> 6 & 3
        return None if width == 0 else key & ~0xC0 | (width - 1) << 6

    pack.table("relative", 2, sorted(
        [row << 16 | key, value] for row, key, value in locales.sparse(fields, shorter)
    ))
    return pack.bytes()


# Intl.DisplayNames dateTimeField codes and the CLDR fields that name them.
DATE_TIME_FIELDS = [
    ("era", "era"), ("year", "year"), ("quarter", "quarter"), ("month", "month"),
    ("weekOfYear", "week"), ("weekday", "weekday"), ("day", "day"),
    ("dayPeriod", "dayperiod"), ("hour", "hour"), ("minute", "minute"),
    ("second", "second"), ("timeZoneName", "zone"),
]
NAME_KINDS = {"languages": 0, "scripts": 1, "territories": 2}
NAME_VARIANTS = {None: 0, "short": 1, "long": 2, "stand-alone": 3}


def names_pack(cldr):
    """Display names: `names[locale << 16 | kind << 4 | variant, code, text]`.

    Kinds: 0 language, 1 script, 2 region, 3 calendar, 4 date-time field
    (variant is the width's index in LENGTHS, code the field's index in
    DATE_TIME_FIELDS), 5 patterns (code 0 "{0} ({1})", code 1 "{0}, {1}").
    For the first four kinds `code` is a row of `codes[text]`, and the
    variants of a name are those of NAME_VARIANTS (CLDR's `alt`).
    """
    pack = Pack()
    locales = Locales(cldr, pack)
    display = path("localeDisplayNames")
    resolved = {}
    codes = set()
    for locale in locales.names:
        entries = {}
        for steps, text in cldr.subtree(locale, display).items():
            tag, attrs = attributes(steps[-1])
            if steps[0] in NAME_KINDS and len(steps) == 2:
                variant = NAME_VARIANTS.get(attrs.get("alt"), None)
                if variant is None:
                    continue
                code = attrs["type"].replace("_", "-")
                entries[(NAME_KINDS[steps[0]] << 4 | variant, code)] = text
            elif steps[0] == "types" and attrs.get("key") == "calendar" and "alt" not in attrs:
                entries[(3 << 4, attrs["type"])] = text
            elif steps[0] == "localeDisplayPattern" and tag in ("localePattern", "localeSeparator"):
                entries[(5 << 4, int(tag == "localeSeparator"))] = text
        for steps, text in cldr.subtree(locale, path("dates", "fields")).items():
            tag, attrs = attributes(steps[0])
            name, _, width = attrs.get("type", "").partition("-")
            fields = [index for index, (_, field) in enumerate(DATE_TIME_FIELDS) if field == name]
            if tag == "field" and fields and steps[1:] == ("displayName",):
                entries[(4 << 4 | LENGTHS.index(width or "long"), fields[0])] = text
        codes.update(code for _, code in entries if isinstance(code, str))
        resolved[locale] = entries
    order = sorted(codes, key=str.encode)
    number = {code: index for index, code in enumerate(order)}
    pack.table("codes", 1, [[pack.s(code)] for code in order])

    def names(locale):
        return {
            (key, number.get(code, code)): pack.s(text)
            for (key, code), text in resolved[locale].items()
        }

    pack.table("names", 3, sorted(
        [row << 16 | key, code, value] for row, (key, code), value in locales.sparse(names)
    ))
    return pack.bytes()


PACKS = {"number.bin": number_pack, "relative.bin": relative_pack, "names.bin": names_pack}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_directory", type=Path)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--only", action="append", help="generate only this pack")
    args = parser.parse_args()
    raw = (args.source_directory / "core.zip").read_bytes()
    if hashlib.sha256(raw).hexdigest() != CORE_ZIP:
        raise SystemExit("unexpected digest for core.zip")
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8")) if MANIFEST.exists() else {}
    outputs = manifest.get("outputs", {})
    failed = False
    for name, build in PACKS.items():
        if args.only and name not in args.only:
            continue
        data = build(Cldr(raw))
        target = DATA / name
        relative = str(target.relative_to(ROOT))
        if args.check:
            if not target.exists() or target.read_bytes() != data:
                print(f"{relative} differs from the pinned CLDR inputs", file=sys.stderr)
                failed = True
        else:
            DATA.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            outputs[relative] = hashlib.sha256(data).hexdigest()
            print(f"{relative}: {len(data)} bytes")
    if failed:
        raise SystemExit(1)
    if not args.check:
        manifest = {
            "license": "Unicode License v3 (crates/rustic-js/data/CLDR-LICENSE.txt)",
            "generator": "tools/generate_js_intl_services.py",
            "sources": {
                "core.zip": {
                    "sha256": CORE_ZIP,
                    "url": "https://github.com/unicode-org/cldr/releases/download/release-48/core.zip",
                },
            },
            "outputs": dict(sorted(outputs.items())),
        }
        MANIFEST.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
