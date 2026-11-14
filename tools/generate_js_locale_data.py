#!/usr/bin/env python3
"""Generate Rustic's locale data from CLDR 48, Unicode 17.0 and IANA tzdb 2026a."""

import argparse
import hashlib
import json
import subprocess
import xml.etree.ElementTree as ET
from io import BytesIO
from pathlib import Path
from zipfile import ZipFile

SOURCES = {
    "core.zip": "06c7c698d6fd8d67cefac15a0206b0109b00e0ef1636f86f84449fa959561f74",
    "supplemental/supplementalData.xml": "fb054c9f9a95f0c4da9dd1e5e0ef6dd208b6f9a73f45303011be247277d81bef",
    "scriptMetadata.txt": "50a0499acddd7a5ce935e8d77b59ab789f91b16c15b95b00236d90137fe662a4",
    "zone1970.tab": "e9d9fe30942a880f756b73f649667d8647a1ecf2131149445d9cc24c65e4ee8f",
    "supplementalMetadata.xml": "36e807ce72b15304dd993f132216f408351be1c4376edaa2fe9e9547e2efcac1",
    "PropList.txt": "130dcddcaadaf071008bdfce1e7743e04fdfbc910886f017d9f9ac931d8c64dd",
    "likelySubtags.xml": "d7732172848cc2870d0d316818190e704bc6614486168dc52af56ee1427b6860",
}
EXTENSION_SOURCES = {
    "bcp47/calendar.xml": "fb24616f77fac1ab3246f869b69efc8a4b86d2437a947f3f6a53843ac0b60821",
    "bcp47/collation.xml": "2fa8fef733069c4651a5c60c85cb38ba09a07151d71b9b148d9372eff28a4d42",
    "bcp47/currency.xml": "93849191705b5e52adab6766d3bf41ae627373c34c7cb73259fdc17d22bd3bc7",
    "bcp47/measure.xml": "6a1445e2f6af8fad47b9d5b40e67d582e7f4b9502609443780d3e3641d23be4d",
    "bcp47/number.xml": "0c10e9578f2191527475d0f57105d56c399b2f44425ceebadf474e50157abbb6",
    "bcp47/segmentation.xml": "9a3a00df3181b51279389655ef06f12be593d3b288c44b54caf7b2d86ed8b285",
    "bcp47/timezone.xml": "54ce7d9f1d385101e24356df7d48ae62cf2acc4421378da60a4007d2488c1aad",
    "bcp47/transform-destination.xml": "469976ddb878572cdede31e94a6e60a4cd9d2d83df9bda3e47045c8489cf76a0",
    "bcp47/transform.xml": "d564dd996541afbcab77b3446bea5e591c3ec5c3b27ed0cd08326ae873ec7bdb",
    "bcp47/variant.xml": "e5500779baa55e838c5bf1529561c3b441fe2c331caac6ceddecffb4e425a514",
}
OUTPUT = (
    Path(__file__).resolve().parents[1]
    / "crates/rustic-js/src/runtime/builtins/intl/data.rs"
)


def read(directory: Path, name: str) -> bytes:
    raw = (directory / name).read_bytes()
    if hashlib.sha256(raw).hexdigest() != (SOURCES | EXTENSION_SOURCES)[name]:
        raise ValueError(f"unexpected locale source digest for {name}")
    return raw


def inherited_values(tag, table, parents):
    while tag != "root":
        if tag in table:
            yield table[tag]
        tag = parents.get(tag, tag.rpartition("-")[0] or "root")
    yield table["root"]


def main_and_collation(directory, supplemental):
    numbering = {}
    collations = {}
    locales = set()
    registered = {}
    data = ET.fromstring(read(directory, "bcp47/collation.xml"))
    for entry in data.findall('.//key[@name="co"]/type'):
        name = entry.attrib["name"]
        if name not in {"search", "standard"} and entry.get("deprecated") != "true":
            registered.update(dict.fromkeys([name, *entry.get("alias", "").split()], name))
    with ZipFile(BytesIO(read(directory, "core.zip"))) as archive:
        for path in archive.namelist():
            folder, _, filename = path.rpartition("/")
            if folder not in {"common/main", "common/collation"} or not filename.endswith(".xml"):
                continue
            tag = filename.removesuffix(".xml").replace("_", "-")
            tree = ET.fromstring(archive.read(path))
            if folder == "common/main":
                locales.add(tag)
                for entry in tree.findall("numbers/defaultNumberingSystem"):
                    if "alt" not in entry.attrib:
                        numbering[tag] = entry.text
                        break
            else:
                collations[tag] = {
                    registered[node.attrib["type"]]
                    for node in tree.findall("collations/collation")
                    if "alt" not in node.attrib and node.attrib["type"] in registered
                }
    parent_tables = {None: {}, "collations": {}}
    for table in supplemental.findall("parentLocales"):
        if table.get("component") in parent_tables:
            parents = parent_tables[table.get("component")]
            for entry in table.findall("parentLocale"):
                parent = entry.attrib["parent"].replace("_", "-")
                for tag in entry.attrib["locales"].split():
                    parents[tag.replace("_", "-")] = parent

    numbers = {"root": numbering["root"]}
    for tag in sorted(locales - {"root"}):
        value = next(inherited_values(tag, numbering, parent_tables[None]))
        prefix = tag.rpartition("-")[0]
        if value != next(inherited_values(prefix, numbers, {})):
            numbers[tag] = value

    result = {"root": sorted(collations["root"])}
    for tag in sorted((locales | collations.keys() | parent_tables["collations"].keys()) - {"root"}):
        values = sorted(set().union(*inherited_values(tag, collations, parent_tables["collations"])))
        prefix = tag.rpartition("-")[0]
        if values != next(inherited_values(prefix, result, {})):
            result[tag] = values
    return numbers, result


def list_patterns(directory, supplemental):
    patterns = {}
    locales = set()
    parents = {}
    for table in supplemental.findall("parentLocales"):
        if "component" in table.attrib:
            continue
        for entry in table.findall("parentLocale"):
            parent = entry.attrib["parent"].replace("_", "-")
            parents.update({tag.replace("_", "-"): parent for tag in entry.attrib["locales"].split()})
    with ZipFile(BytesIO(read(directory, "core.zip"))) as archive:
        for path in archive.namelist():
            if not path.startswith("common/main/") or not path.endswith(".xml"):
                continue
            root = ET.fromstring(archive.read(path))
            tag = path.rpartition("/")[2].removesuffix(".xml").replace("_", "-")
            locales.add(tag)
            values = {}
            for pattern in root.findall("listPatterns/listPattern"):
                kind = pattern.get("type", "standard")
                for entry in pattern.findall("listPatternPart"):
                    if entry.get("alt") is None and entry.text not in ("↑↑↑", "∅∅∅"):
                        template = entry.text
                        if (template.count("{0}") != 1 or template.count("{1}") != 1
                                or template.index("{0}") > template.index("{1}")):
                            raise ValueError(f"unsupported list template in {tag}: {template!r}")
                        values[kind, entry.attrib["type"]] = template
            if values:
                patterns[tag] = values

    root_aliases = {
        "or-narrow": "or-short", "or-short": "or",
        "standard-narrow": "standard-short", "standard-short": "standard",
        "unit": "unit-short", "unit-narrow": "unit-short", "unit-short": "standard",
    }

    def resolve(tag, kind, part):
        while True:
            candidate = tag
            while True:
                value = patterns.get(candidate, {}).get((kind, part))
                if value is not None:
                    return value
                if candidate == "root":
                    break
                candidate = parents.get(candidate, candidate.rpartition("-")[0] or "root")
            # Root's local aliases restart lookup at the requested locale.
            kind = root_aliases[kind]

    def rust_string(value):
        return json.dumps(value, ensure_ascii=False).replace("\u200b", r"\u{200b}")

    table = []
    groups = {}
    # Include data-inheriting locales so a request can resolve to that locale
    # even when its templates are identical to its parent.
    for tag in sorted(locales - {"root", "zxx"}):
        group = tuple(
            resolve(tag, base + suffix, part)
            for base in ("standard", "or", "unit")
            for suffix in ("", "-short", "-narrow")
            for part in ("2", "start", "middle", "end")
        )
        number = groups.setdefault(group, len(groups))
        table.append((tag, number))
    lines = ["pub(super) const LIST_LOCALES: &[(&str, usize)] = &["]
    lines.extend(f'    ("{tag}", {number}),' for tag, number in table)
    lines.extend(("];", "", "pub(super) const LIST_PATTERNS: &[[[&str; 4]; 9]] = &["))
    for group in groups:
        lines.append("    [")
        for index in range(0, len(group), 4):
            entries = ", ".join(rust_string(value) for value in group[index:index + 4])
            lines.append(f"        [{entries}],")
        lines.append("    ],")
    return [*lines, "];", ""]


def preference_data(directory):
    supplemental = ET.fromstring(read(directory, "supplemental/supplementalData.xml"))
    calendars = {}
    for entry in supplemental.findall("calendarPreferenceData/calendarPreference"):
        values = ["gregory" if item == "gregorian" else item for item in entry.attrib["ordering"].split()]
        for region in entry.attrib["territories"].split():
            calendars[region] = values
    hours = {}
    hour_cycles = {"K": "h11", "h": "h12", "H": "h23", "k": "h24"}
    for entry in supplemental.findall("timeData/hours"):
        order = [entry.attrib["preferred"], *entry.attrib["allowed"].split()]
        values = list(dict.fromkeys(hour_cycles[item[0]] for item in order))
        for region in entry.attrib["regions"].split():
            hours[region.replace("_", "-")] = values
    zone_sets = {}
    for line in read(directory, "zone1970.tab").decode("utf-8").splitlines():
        if line and not line.startswith("#"):
            countries, _, zone, *_ = line.split("\t")
            for country in countries.split(","):
                zone_sets.setdefault(country, set()).add(zone)
    zones = {country: sorted(names) for country, names in zone_sets.items()}
    numbers, collations = main_and_collation(directory, supplemental)
    lines = []
    for name, table in [("CALENDARS", calendars), ("HOUR_CYCLES", hours),
                        ("TIME_ZONES", zones), ("COLLATIONS", collations)]:
        if name == "TIME_ZONES":
            lines.append("// Public-domain IANA tzdb 2026a zone1970.tab.")
        lines.append(f"pub(super) const {name}: &[(&str, &[&str])] = &[")
        for key, values in sorted(table.items()):
            items = ", ".join(f'"{value}"' for value in values)
            lines.append(f'    ("{key}", &[{items}]),')
        lines.extend(("];", ""))
    lines.append("pub(super) const NUMBERING_SYSTEMS: &[(&str, &str)] = &[")
    lines.extend(f'    ("{key}", "{value}"),' for key, value in sorted(numbers.items()) if key != "root")
    lines.extend(("];", ""))

    weekdays = dict(zip(["mon", "tue", "wed", "thu", "fri", "sat", "sun"], range(1, 8)))
    week_tables = [{
        territory: weekdays[entry.attrib["day"]]
        for entry in supplemental.findall(f"weekData/{name}")
        if "alt" not in entry.attrib
        for territory in entry.attrib["territories"].split()
    } for name in ("firstDay", "weekendStart", "weekendEnd")]
    lines.append("pub(super) const WEEK_DATA: &[(&str, [u8; 3])] = &[")
    for territory in sorted(set().union(*(table.keys() for table in week_tables))):
        values = [table.get(territory, table["001"]) for table in week_tables]
        lines.append(f'    ("{territory}", {values}),')
    lines.extend(("];", ""))

    directions = {}
    for line in read(directory, "scriptMetadata.txt").decode("utf-8").splitlines():
        fields = [field.strip() for field in line.split("#", 1)[0].split(";")]
        if len(fields) > 6 and fields[6] in ("YES", "NO"):
            directions[fields[0]] = "rtl" if fields[6] == "YES" else "ltr"
    lines.append("pub(super) const SCRIPT_DIRECTIONS: &[(&str, &str)] = &[")
    lines.extend(f'    ("{key}", "{value}"),' for key, value in sorted(directions.items()))
    lines.extend(("];", ""))
    lines.extend(list_patterns(directory, supplemental))
    return lines


def generate(directory: Path) -> str:
    metadata = ET.fromstring(read(directory, "supplementalMetadata.xml"))
    likely = ET.fromstring(read(directory, "likelySubtags.xml"))
    lines = [
        "// Generated from Unicode CLDR release 48 by tools/generate_js_locale_data.py.",
        "// Unicode License V3. See crates/rustic-js/data/CLDR-LICENSE.txt.",
        "",
    ]
    territories = {
        region.lower()
        for alias in metadata.findall(".//territoryAlias")
        if len(alias.attrib["replacement"].split()) > 1
        for region in alias.attrib["replacement"].split()
    }
    variant_languages = []
    for constant, node, key, value in [
        ("LANGUAGES", metadata.findall(".//languageAlias"), "type", "replacement"),
        ("SCRIPTS", metadata.findall(".//scriptAlias"), "type", "replacement"),
        ("REGIONS", metadata.findall(".//territoryAlias"), "type", "replacement"),
        ("SUBDIVISIONS", metadata.findall(".//subdivisionAlias"), "type", "replacement"),
        ("VARIANTS", metadata.findall(".//variantAlias"), "type", "replacement"),
        ("LIKELY_REGIONS", likely.findall(".//likelySubtag"), "from", "to"),
    ]:
        pairs = {}
        for element in node:
            raw = element.attrib[key]
            replacement = element.attrib[value]
            if constant == "LANGUAGES":
                name, *variants = raw.lower().split("_")
                if variants and all(len(variant) >= 5 for variant in variants):
                    variant_languages.append((name, sorted(variants), replacement.lower().replace("_", "-")))
                    continue
            if constant == "LIKELY_REGIONS":
                if len(raw.split("_")) > 2 or raw.split("_")[-1].isupper():
                    continue
                replacement = replacement.rsplit("_", 1)[1]
                if replacement.lower() not in territories:
                    continue
            pairs[raw.lower().replace("_", "-")] = replacement.lower().replace("_", "-")
        lines.append(f"pub(super) const {constant}: &[(&str, &str)] = &[")
        lines.extend(f'    ("{key}", "{value}"),' for key, value in sorted(pairs.items()))
        lines.extend(("];", ""))
    lines.append("pub(super) const VARIANT_LANGUAGES: &[(&str, &[&str], &str)] = &[")
    for name, variants, replacement in sorted(
        variant_languages, key=lambda row: (row[0] == "und", -len(row[1]), row)
    ):
        patterns = ", ".join(f'"{variant}"' for variant in variants)
        lines.append(f'    ("{name}", &[{patterns}], "{replacement}"),')
    lines.extend(("];", ""))
    lines.append("pub(super) const LIKELY_SUBTAGS: &[(&str, &str)] = &[")
    pairs = [
        (entry.attrib["from"].lower().replace("_", "-"), entry.attrib["to"].replace("_", "-"))
        for entry in likely.findall(".//likelySubtag")
    ]
    lines.extend(f'    ("{key}", "{value}"),' for key, value in sorted(pairs))
    lines.extend(("];", ""))
    lines.extend(preference_data(directory))
    aliases = {}
    for name in EXTENSION_SOURCES:
        for key in ET.fromstring(read(directory, name)).findall(".//key"):
            identifier = key.attrib["name"]
            for entry in key.findall("type"):
                canonical = entry.attrib["name"].lower()
                preferred = entry.get("preferred", canonical).lower()
                if preferred != canonical:
                    aliases[f"{identifier}-{canonical}"] = preferred
                for spelling in entry.get("alias", "").lower().split():
                    if all(part.isalnum() and 3 <= len(part) <= 8 for part in spelling.split("-")):
                        aliases[f"{identifier}-{spelling}"] = preferred
    lines.append("pub(super) const EXTENSION_ALIASES: &[(&str, &str)] = &[")
    lines.extend(f'    ("{key}", "{value}"),' for key, value in sorted(aliases.items()))
    lines.extend(("];", ""))
    lines.append("// Unicode 17.0.0 PropList.txt, Soft_Dotted.")
    lines.append("pub(super) const SOFT_DOTTED: &[(u32, u32)] = &[")
    for line in read(directory, "PropList.txt").decode("utf-8").splitlines():
        fields = [part.strip() for part in line.split("#", 1)[0].split(";")]
        if len(fields) != 2 or fields[1] != "Soft_Dotted":
            continue
        first, _, last = fields[0].partition("..")
        lines.append(f"    (0x{first}, 0x{last or first}),")
    lines.append("];")
    return "\n".join(lines).rstrip() + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_directory", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    generated = subprocess.run(
        ["rustfmt", "--edition", "2024", "--emit", "stdout"],
        input=generate(args.source_directory),
        capture_output=True,
        text=True,
        check=True,
        cwd=Path(__file__).resolve().parents[1],
    ).stdout
    if args.check:
        if OUTPUT.read_text(encoding="utf-8") != generated:
            raise SystemExit("locale data differs from the pinned CLDR inputs")
    else:
        OUTPUT.write_text(generated, encoding="utf-8")


if __name__ == "__main__":
    main()
