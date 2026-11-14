#!/usr/bin/env python3
"""Generate Rustic's locale alias data from Unicode CLDR release 48."""

import argparse
import hashlib
import subprocess
import xml.etree.ElementTree as ET
from pathlib import Path

SOURCES = {
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
        raise ValueError(f"unexpected CLDR 48 digest for {name}")
    return raw


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
