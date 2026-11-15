#!/usr/bin/env python3
"""Generate the data packs of Rustic's Intl services from CLDR 48.

Takes the same pinned source directory as tools/generate_js_locale_data.py,
with the Unicode files of UCD_FILES in its `ucd/` folder, and writes crates/rustic-js/data/intl/*.bin with their digests in
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
from js_date_time_data import pack_date_time  # noqa: E402

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


# Unicode 17.0.0 files for text segmentation, from the unicodetools mirror.
UCD = "https://raw.githubusercontent.com/unicode-org/unicodetools/63aff0ca8dea894a057d04e7aef7ae1241757680/unicodetools/data/ucd/17.0.0"
UCD_FILES = {
    "GraphemeBreakProperty.txt": ("auxiliary/", "d6b51d1d2ae5c33b451b7ed994b48f1f4dc62b2272a5831e7fd418514a6bae89"),
    "WordBreakProperty.txt": ("auxiliary/", "72274cac1e6b919507db35655c3e175aa27274668a1ece95c28d2069f2ad9852"),
    "SentenceBreakProperty.txt": ("auxiliary/", "871c0c985ad95125e25b302414065a10839d068970bceb383ecec138f22a0a18"),
    "emoji-data.txt": ("emoji/", "2cb2bb9455cda83e8481541ecf5b6dfda66a3bb89efa3fa7c5297eccf607b72b"),
    "DerivedCoreProperties.txt": ("", "24c7fed1195c482faaefd5c1e7eb821c5ee1fb6de07ecdbaa64b56a99da22c08"),
    "GraphemeBreakTest.txt": ("auxiliary/", "e2d134d2c52919bace503ebb6a551c1855fe1a1faec18478c78fff254a1793ec"),
    "WordBreakTest.txt": ("auxiliary/", "1de23a75f37904abc7d206239ee8d34f8fdf0fb4ab32a7174dfbabbde25419b2"),
    "SentenceBreakTest.txt": ("auxiliary/", "12cb47d028ded0c1cb8a28558f95479cbcd24559c46977015c82f3b50a1cc6e4"),
}
# Class numbers shared with intl/segment.rs.
GRAPHEME = ["Other", "CR", "LF", "Control", "Extend", "ZWJ", "Regional_Indicator", "Prepend",
            "SpacingMark", "L", "V", "T", "LV", "LVT"]
WORD = ["Other", "CR", "LF", "Newline", "Extend", "ZWJ", "Regional_Indicator", "Format",
        "Katakana", "Hebrew_Letter", "ALetter", "Single_Quote", "Double_Quote", "MidNumLet",
        "MidLetter", "MidNum", "Numeric", "ExtendNumLet", "WSegSpace"]
SENTENCE = ["Other", "CR", "LF", "Extend", "Sep", "Format", "Sp", "Lower", "Upper", "OLetter",
            "Numeric", "ATerm", "SContinue", "STerm", "Close"]
INCB = ["None", "Consonant", "Extend", "Linker"]
SEGMENT_CASES = ROOT / "crates/rustic-js/tests/fixtures/segmentation_cases.txt"


def ucd(directory, name):
    raw = (directory / "ucd" / name).read_bytes()
    if hashlib.sha256(raw).hexdigest() != UCD_FILES[name][1]:
        raise SystemExit(f"unexpected digest for ucd/{name}")
    return raw.decode("utf-8")


def ucd_lines(text):
    """`(first, last, fields)` of each data line of a UCD property file."""
    for line in text.splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            span, *fields = [field.strip() for field in line.split(";")]
            first, _, last = span.partition("..")
            yield int(first, 16), int(last or first, 16), fields


def segmentation_cases(directory):
    """The UAX #29 conformance tests, one per line: a kind letter and the
    code points with `/` at each boundary."""
    lines = [
        "# Unicode 17.0.0 GraphemeBreakTest, WordBreakTest and SentenceBreakTest.",
        "# Generated by tools/generate_js_intl_services.py; do not edit.",
    ]
    for kind, name in (("g", "GraphemeBreakTest.txt"), ("w", "WordBreakTest.txt"),
                       ("s", "SentenceBreakTest.txt")):
        for line in ucd(directory, name).splitlines():
            line = line.split("#", 1)[0].strip()
            if line:
                marks = line.replace("÷", "/").replace("×", "").split()
                lines.append(f"{kind} {' '.join(marks)}")
    return "\n".join(lines) + "\n"


def segment_pack(cldr, directory):
    """Break properties: `breaks[first code point, classes]`, one row per
    run of code points with the same classes. `classes` packs the indices
    into GRAPHEME (bits 0-3), WORD (4-8), SENTENCE (9-12) and INCB
    (14-15), Extended_Pictographic (bit 13) and Alphabetic (bit 16).
    """
    del cldr
    values = [0] * 0x110000

    def assign(name, names, shift, column=0, wanted=None):
        for first, last, fields in ucd_lines(ucd(directory, name)):
            if wanted is not None and fields[0] != wanted:
                continue
            index = names.index(fields[column]) if names else 1
            for code in range(first, last + 1):
                values[code] |= index << shift

    assign("GraphemeBreakProperty.txt", GRAPHEME, 0)
    assign("WordBreakProperty.txt", WORD, 4)
    assign("SentenceBreakProperty.txt", SENTENCE, 9)
    assign("emoji-data.txt", None, 13, wanted="Extended_Pictographic")
    assign("DerivedCoreProperties.txt", INCB, 14, column=1, wanted="InCB")
    assign("DerivedCoreProperties.txt", None, 16, wanted="Alphabetic")
    pack = Pack()
    rows = []
    for code, value in enumerate(values):
        if not rows or rows[-1][1] != value:
            rows.append([code, value])
    pack.table("breaks", 2, rows)
    return pack.bytes()


def date_time_pack(cldr, _):
    return pack_date_time(cldr, Locales)


PACKS = {
    "date-time.bin": date_time_pack,
    "number.bin": lambda cldr, directory: number_pack(cldr),
    "relative.bin": lambda cldr, directory: relative_pack(cldr),
    "names.bin": lambda cldr, directory: names_pack(cldr),
    "segment.bin": segment_pack,
}


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
        data = build(Cldr(raw), args.source_directory)
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
    if not args.only or "segment.bin" in args.only:
        cases = segmentation_cases(args.source_directory)
        if args.check:
            if not SEGMENT_CASES.exists() or SEGMENT_CASES.read_text(encoding="utf-8") != cases:
                print(f"{SEGMENT_CASES.relative_to(ROOT)} differs from the pinned inputs", file=sys.stderr)
                failed = True
        else:
            SEGMENT_CASES.write_text(cases, encoding="utf-8")
            outputs[str(SEGMENT_CASES.relative_to(ROOT))] = hashlib.sha256(cases.encode()).hexdigest()
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
                **{
                    f"ucd/{name}": {"sha256": digest, "url": f"{UCD}/{folder}{name}"}
                    for name, (folder, digest) in UCD_FILES.items()
                },
            },
            "outputs": dict(sorted(outputs.items())),
        }
        MANIFEST.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
