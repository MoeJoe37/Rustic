#!/usr/bin/env python3
"""Render Rustic's evidence-backed whole-engine capability progress map."""

import argparse
from fractions import Fraction
import hashlib
from html import escape
import json
import os
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
START = "<!-- rustic-progress:start -->"
END = "<!-- rustic-progress:end -->"
OUTPUTS = ("docs/progress/engine.svg", "docs/progress/data.json", "docs/progress/summary.md")
IGNORED = {".git", "target", "artifacts", "__pycache__", "checkouts", ".idea", ".vscode"}
GROUPS = {"base", "url", "dom", "html", "css", "text", "layout", "paint", "js", "wasm", "web", "network", "storage", "security", "compositor", "platform", "a11y", "devtools", "extensions", "filtering", "release"}
REQUIRED = {"release.complete", "platform.android", "platform.ios", "platform.linux", "platform.windows", "platform.macos", "js.conformance", "js.wasm", "security.audit", "html.conformance", "css.conformance", "layout.incremental", "compositor.parity", "wasm.conformance", "extensions.conformance", "filtering.acceptance", "security.brave", "security.privacy", "security.updates", "release.comparison"}


def percent(value: Fraction) -> str:
    # Truncate instead of rounding a nearly complete engine up to 100%.
    whole, remainder = divmod(value.numerator * 100 // value.denominator, 100)
    return f"{whole}.{remainder:02d}"


def normalize_readme(text: str) -> str:
    if text.count(START) != 1 or text.count(END) != 1 or text.index(START) >= text.index(END):
        raise ValueError("README must contain one ordered progress marker pair")
    before, rest = text.split(START)
    _, after = rest.split(END)
    return before + START + END + after


def validate(catalog: dict, root: Path) -> None:
    if catalog.get("schema_version") != 1 or not re.fullmatch(r"engine-v[1-9][0-9]*", catalog.get("catalog_version", "")):
        raise ValueError("unsupported progress schema/catalog version")
    groups = catalog.get("groups", [])
    if {group.get("id") for group in groups} != GROUPS or len(groups) != len(GROUPS):
        raise ValueError(f"catalog must retain all {len(GROUPS)} engine subsystems")
    if any(type(group.get("weight")) is not int or group["weight"] <= 0 for group in groups) or sum(group["weight"] for group in groups) != 100:
        raise ValueError("positive integer subsystem weights must sum to 100")
    identifiers = set()
    for group in groups:
        if not isinstance(group.get("label"), str) or not group["label"] or not group.get("features"):
            raise ValueError("every subsystem needs a label and nonempty capability list")
        for feature in group["features"]:
            identifier = feature.get("id", "")
            if not re.fullmatch(re.escape(group["id"]) + r"\.[a-z0-9_.-]+", identifier) or identifier in identifiers:
                raise ValueError(f"invalid/duplicate capability ID: {identifier}")
            identifiers.add(identifier)
            if not isinstance(feature.get("label"), str) or not feature["label"]:
                raise ValueError(f"missing capability label: {identifier}")
            if feature.get("status") not in {"planned", "in_progress", "complete"}:
                raise ValueError(f"invalid capability status: {identifier}")
            evidence = feature.get("evidence", [])
            if feature["status"] == "complete" and {entry.get("kind") for entry in evidence} != {"source", "test"}:
                raise ValueError(f"completed capability needs source and test evidence: {identifier}")
            for entry in evidence:
                path = (root / entry["path"]).resolve()
                marker = entry.get("marker")
                if entry.get("kind") not in {"source", "test"} or not path.is_relative_to(root.resolve()) or not path.is_file():
                    raise ValueError(f"invalid evidence path for {identifier}")
                if not isinstance(marker, str) or not marker or marker not in path.read_text(encoding="utf-8"):
                    raise ValueError(f"missing evidence marker for {identifier}: {entry['path']}")
    if not REQUIRED.issubset(identifiers):
        raise ValueError("catalog is missing mandatory full-engine/platform acceptance gates")


def source_fingerprint(root: Path) -> str:
    digest = hashlib.sha256()
    files = []
    for directory, dirs, names in os.walk(root):
        dirs[:] = [name for name in dirs if name not in IGNORED]
        files.extend(Path(directory) / name for name in names if name != ".DS_Store")
    for path in sorted(files):
        relative = path.relative_to(root).as_posix()
        if relative in OUTPUTS:
            continue
        if path.is_symlink():
            raise ValueError(f"source snapshot does not follow symlinks: {relative}")
        content = path.read_bytes()
        if relative == "README.md":
            content = normalize_readme(content.decode("utf-8")).encode("utf-8")
        digest.update(relative.encode() + b"\0" + hashlib.sha256(content).digest())
    return digest.hexdigest()


def measure(catalog: dict, fingerprint: str) -> dict:
    groups = []
    overall = Fraction(0)
    for group in catalog["groups"]:
        done = sum(feature["status"] == "complete" for feature in group["features"])
        total = len(group["features"])
        overall += Fraction(group["weight"] * done, total)
        groups.append({**group, "completed": done, "total": total, "percent": percent(Fraction(100 * done, total))})
    completed = sum(group["completed"] for group in groups)
    total = sum(group["total"] for group in groups)
    return {"schema_version": 1, "catalog_version": catalog["catalog_version"],
            "source_fingerprint": fingerprint, "percent": percent(overall),
            "completed": completed, "total": total, "fully_complete": completed == total,
            "meaning": catalog["meaning"], "groups": groups}


def render_svg(data: dict) -> str:
    rows = (len(data["groups"]) + 2) // 3
    legend_y = 128 + rows * 84
    height = legend_y + 88
    lines = [f'<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="{height}" viewBox="0 0 1000 {height}" role="img" aria-labelledby="title desc">',
             f'<title id="title">Rustic engine completion: {data["percent"]}%</title>',
             '<desc id="desc">Green cells are completed, evidence-backed capabilities; amber is work in progress and gray is planned. Overall completion weights every engine subsystem, including final five-platform release gates.</desc>',
             f'<rect width="1000" height="{height}" rx="12" fill="#0d1117"/>',
             '<g font-family="DejaVu Sans,Arial,sans-serif">',
             '<text x="24" y="36" fill="#f0f6fc" font-size="22" font-weight="700">Rustic · engine subsystem progress</text>',
             f'<text x="976" y="39" fill="#3fb950" font-size="28" font-weight="700" text-anchor="end">{data["percent"]}%</text>',
             '<rect x="24" y="56" width="952" height="10" rx="5" fill="#30363d"/>',
             f'<rect x="24" y="56" width="{float(Fraction(data["percent"]) * Fraction(952, 100)):.2f}" height="10" rx="5" fill="#3fb950"/>',
             f'<text x="24" y="91" fill="#8b949e" font-size="12">{data["completed"]}/{data["total"]} capabilities complete · {len(data["groups"])} subsystems · {escape(data["catalog_version"])}</text>']
    colors = {"complete": "#3fb950", "in_progress": "#d29922", "planned": "#30363d"}
    for index, group in enumerate(data["groups"]):
        x, y = 24 + (index % 3) * 322, 110 + (index // 3) * 84
        label = escape(group["label"])
        lines += [f'<g><title>{label}: {group["completed"]}/{group["total"]} capabilities, {group["weight"]}% of engine scope</title>',
                  f'<rect x="{x}" y="{y}" width="308" height="74" rx="6" fill="#161b22" stroke="#30363d"/>',
                  f'<text x="{x+12}" y="{y+22}" fill="#f0f6fc" font-size="13" font-weight="600">{label}</text>',
                  f'<text x="{x+296}" y="{y+22}" fill="#8b949e" text-anchor="end" font-size="12">{group["percent"]}%</text>']
        step = min(17, 282 / len(group["features"]))
        for position, feature in enumerate(group["features"]):
            color = colors[feature["status"]]
            lines.append(f'<rect x="{x+12+position*step:.2f}" y="{y+34}" width="{step-3:.2f}" height="12" rx="2" fill="{color}"><title>{escape(feature["label"])}: {feature["status"]}</title></rect>')
        lines += [f'<text x="{x+12}" y="{y+62}" fill="#8b949e" font-size="10">{group["completed"]}/{group["total"]} complete · scope weight {group["weight"]}%</text>', '</g>']
    for x, color, label in [(24, "#3fb950", "Complete + evidence"), (220, "#d29922", "In progress"), (370, "#30363d", "Planned")]:
        lines += [f'<rect x="{x}" y="{legend_y}" width="12" height="12" rx="2" fill="{color}"/>', f'<text x="{x+20}" y="{legend_y+11}" fill="#c9d1d9" font-size="12">{label}</text>']
    lines += [f'<text x="24" y="{legend_y+40}" fill="#c9d1d9" font-size="12">100% requires every capability and final engine acceptance on Windows, macOS, Linux, Android and iOS.</text>',
              f'<text x="24" y="{legend_y+64}" fill="#8b949e" font-size="11">Weighted delivery progress · not a web-compatibility percentage · source snapshot {data["source_fingerprint"][:12]}</text>', '</g>', '</svg>']
    return "\n".join(lines) + "\n"


def summary(data: dict) -> str:
    lines = ["# Engine progress snapshot", "", f"**Overall completion: {data['percent']}%** · {data['completed']}/{data['total']} capabilities complete.", "",
             f"Catalog: `{data['catalog_version']}`. Source fingerprint: `{data['source_fingerprint']}`.", "",
             "100% requires all capabilities and final five-platform engine acceptance. Weights measure planned engine scope; they are not compatibility results or time estimates.", "",
             "| Subsystem | Completed | Subsystem progress | Engine weight |", "| --- | --- | --- | --- |"]
    for group in data["groups"]:
        lines.append(f"| {group['label']} | {group['completed']}/{group['total']} | {group['percent']}% | {group['weight']}% |")
    lines += ["", "## Capability evidence", ""]
    for group in data["groups"]:
        lines.extend([f"### {group['label']}", ""])
        for feature in group["features"]:
            proof = ", ".join(f"[{entry['kind']}](../../{entry['path']})" for entry in feature["evidence"])
            lines.append(f"- **{feature['status']}** `{feature['id']}` — {feature['label']}" + (f" ({proof})" if proof else ""))
        lines.append("")
    return "\n".join(lines)


def readme_block(data: dict) -> str:
    return "\n".join([START, "", "## Engine progress", "",
        "[![Rustic engine subsystem progress](docs/progress/engine.svg)](docs/progress/summary.md)", "",
        f"**Overall engine completion: {data['percent']}%** — {data['completed']}/{data['total']} capabilities complete across {len(data['groups'])} subsystems.", "",
        "100% requires the full capability catalog and final engine acceptance on Windows, macOS, Linux, Android and iOS. This is weighted delivery progress; see [measurement and automatic updates](docs/PROGRESS.md).", "", END])


def generate(root: Path) -> tuple[dict, dict[Path, str]]:
    catalog = json.loads((root / "progress/catalog.json").read_text(encoding="utf-8"))
    validate(catalog, root)
    data = measure(catalog, source_fingerprint(root))
    readme = (root / "README.md").read_text(encoding="utf-8")
    before, rest = readme.split(START)
    _, after = rest.split(END)
    return data, {root / OUTPUTS[0]: render_svg(data), root / OUTPUTS[1]: json.dumps(data, indent=2) + "\n",
                  root / OUTPUTS[2]: summary(data), root / "README.md": before + readme_block(data) + after}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--write", action="store_true")
    mode.add_argument("--check", action="store_true")
    args = parser.parse_args()
    try:
        data, outputs = generate(args.root.resolve())
        stale = [str(path.relative_to(args.root.resolve())) for path, content in outputs.items() if not path.exists() or path.read_text(encoding="utf-8") != content]
        if args.check and stale:
            raise ValueError("stale progress outputs: " + ", ".join(stale) + "; run python3 tools/progress.py --write")
        if args.write:
            for path, content in outputs.items():
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content, encoding="utf-8", newline="\n")
        print(f"Engine progress: {data['percent']}% ({data['completed']}/{data['total']} capabilities); snapshot {data['source_fingerprint'][:12]}")
        return 0
    except (OSError, ValueError, KeyError, TypeError) as exc:
        print(f"Progress error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
