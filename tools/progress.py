#!/usr/bin/env python3
"""Render Rustic's evidence-backed, scope-weighted whole-engine progress map."""

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
GROUPS = {"base", "url", "dom", "html", "css", "text", "layout", "paint", "js", "wasm", "web", "media", "graphics", "devices", "network", "storage", "security", "compositor", "platform", "a11y", "devtools", "extensions", "filtering", "perf", "release"}
SIZES = {1, 2, 3, 5, 8}
SHORT = {"base": "Base", "url": "URL", "dom": "DOM", "html": "HTML", "css": "CSS", "text": "Text", "layout": "Layout", "paint": "Paint", "js": "JS", "wasm": "Wasm", "web": "Web", "media": "Media", "graphics": "Gfx", "devices": "Device", "network": "Net", "storage": "Store", "security": "Sec", "compositor": "GPU", "platform": "Host", "a11y": "A11y", "devtools": "Dev", "extensions": "Ext", "filtering": "uBO", "perf": "Perf", "release": "Rel"}
REQUIRED = {"release.complete", "platform.android", "platform.ios", "platform.linux", "platform.windows", "platform.macos", "js.conformance", "js.wasm", "security.audit", "html.conformance", "css.conformance", "layout.incremental", "compositor.parity", "wasm.conformance", "extensions.conformance", "filtering.acceptance", "security.brave", "security.privacy", "security.updates", "release.comparison",
            "media.conformance", "graphics.conformance", "devices.conformance", "perf.gate", "release.inventory", "release.interop"}


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


def check_evidence(evidence: list, root: Path, identifier: str) -> None:
    for entry in evidence:
        path = (root / entry.get("path", "")).resolve()
        marker = entry.get("marker")
        if entry.get("kind") not in {"source", "test"} or not path.is_relative_to(root.resolve()) or not path.is_file():
            raise ValueError(f"invalid evidence path for {identifier}")
        if not isinstance(marker, str) or not marker or marker not in path.read_text(encoding="utf-8"):
            raise ValueError(f"missing evidence marker for {identifier}: {entry['path']}")


def proven(evidence: list) -> bool:
    return {entry.get("kind") for entry in evidence} == {"source", "test"}


def validate(catalog: dict, root: Path) -> None:
    if catalog.get("schema_version") != 2 or not re.fullmatch(r"engine-v[1-9][0-9]*", catalog.get("catalog_version", "")):
        raise ValueError("unsupported progress schema/catalog version")
    milestones = catalog.get("milestones", [])
    milestone_ids = [milestone.get("id") for milestone in milestones]
    if milestone_ids != [f"M{index}" for index in range(1, len(milestones) + 1)] or not milestones:
        raise ValueError("milestones must be listed in order as M1, M2, ...")
    if any(not isinstance(milestone.get("label"), str) or not milestone["label"] for milestone in milestones):
        raise ValueError("every milestone needs a label")
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
            if type(feature.get("size")) is not int or feature["size"] not in SIZES:
                raise ValueError(f"capability size must be one of {sorted(SIZES)}: {identifier}")
            if feature.get("milestone") not in milestone_ids:
                raise ValueError(f"unknown capability milestone: {identifier}")
            status = feature.get("status")
            if status not in {"planned", "in_progress", "complete"}:
                raise ValueError(f"invalid capability status: {identifier}")
            evidence = feature.get("evidence", [])
            if status == "complete" and not proven(evidence):
                raise ValueError(f"completed capability needs source and test evidence: {identifier}")
            check_evidence(evidence, root, identifier)
            steps = feature.get("steps")
            if steps is None:
                continue
            if status != "in_progress" or not isinstance(steps, list) or len(steps) < 2:
                raise ValueError(f"only in-progress capabilities may list two or more steps: {identifier}")
            for step in steps:
                if not isinstance(step.get("label"), str) or not step["label"] or step.get("status") not in {"planned", "complete"}:
                    raise ValueError(f"every step needs a label and planned/complete status: {identifier}")
                if step["status"] == "complete" and not proven(step.get("evidence", [])):
                    raise ValueError(f"completed step needs source and test evidence: {identifier}")
                check_evidence(step.get("evidence", []), root, identifier)
            if all(step["status"] == "complete" for step in steps):
                raise ValueError(f"every step is complete; mark the capability complete: {identifier}")
    if not REQUIRED.issubset(identifiers):
        raise ValueError("catalog is missing mandatory full-engine/platform acceptance gates")


def credit(feature: dict) -> Fraction:
    """Completion credit for one capability: 1, a verified step fraction, or 0."""
    if feature["status"] == "complete":
        return Fraction(1)
    if feature["status"] == "in_progress" and feature.get("steps"):
        return Fraction(sum(step["status"] == "complete" for step in feature["steps"]), len(feature["steps"]))
    return Fraction(0)


def source_fingerprint(root: Path) -> str:
    digest = hashlib.sha256()
    files = []
    for directory, dirs, names in os.walk(root):
        dirs[:] = [name for name in dirs if name not in IGNORED]
        files.extend(Path(directory) / name for name in names if name not in {".DS_Store", ".git"})
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


def split(done: Fraction, verified: Fraction) -> dict:
    """Truncated display values whose verified + partial parts add up exactly."""
    shown, shown_verified = percent(done), percent(verified)
    return {"percent": shown, "verified_percent": shown_verified,
            "partial_percent": percent(Fraction(shown) - Fraction(shown_verified))}


def measure(catalog: dict, fingerprint: str) -> dict:
    groups = []
    overall = verified = Fraction(0)
    milestones = {milestone["id"]: {**milestone, "share": Fraction(0), "done": Fraction(0), "verified": Fraction(0),
                                    "completed": 0, "total": 0} for milestone in catalog["milestones"]}
    for group in catalog["groups"]:
        points = sum(feature["size"] for feature in group["features"])
        features, group_done, group_verified = [], Fraction(0), Fraction(0)
        for feature in group["features"]:
            share = Fraction(group["weight"] * feature["size"], points)
            earned = credit(feature)
            whole = Fraction(feature["status"] == "complete")
            group_done += share * earned
            group_verified += share * whole
            milestone = milestones[feature["milestone"]]
            milestone["share"] += share
            milestone["done"] += share * earned
            milestone["verified"] += share * whole
            milestone["completed"] += feature["status"] == "complete"
            milestone["total"] += 1
            features.append({**feature, "credit": percent(100 * earned), "engine_share": percent(share)})
        overall += group_done
        verified += group_verified
        weight = Fraction(group["weight"])
        groups.append({**group, "features": features, "points": points,
                       "completed": sum(feature["status"] == "complete" for feature in features),
                       "in_progress": sum(feature["status"] == "in_progress" for feature in features),
                       "total": len(features), "contribution": percent(group_done),
                       **split(100 * group_done / weight, 100 * group_verified / weight)})
    milestone_rows = []
    for milestone in milestones.values():
        share = milestone["share"]
        if not share:
            continue
        milestone_rows.append({"id": milestone["id"], "label": milestone["label"], "share": percent(share),
                               "completed": milestone["completed"], "total": milestone["total"],
                               **split(100 * milestone["done"] / share, 100 * milestone["verified"] / share)})
    completed = sum(group["completed"] for group in groups)
    total = sum(group["total"] for group in groups)
    return {"schema_version": 2, "catalog_version": catalog["catalog_version"],
            "source_fingerprint": fingerprint, **split(overall, verified),
            "completed": completed, "in_progress": sum(group["in_progress"] for group in groups),
            "total": total, "fully_complete": completed == total,
            "meaning": catalog["meaning"], "milestones": milestone_rows, "groups": groups}


GREEN, BLUE, TRACK, EMPTY = "#2ea043", "#388bfd", "#21262d", "#30363d"
INK, SECONDARY, MUTED, CARD, LINE = "#f0f6fc", "#c9d1d9", "#8b949e", "#161b22", "#30363d"


def bar(x: float, y: float, width: float, height: float, done: str, verified: str, radius: float = 3) -> list[str]:
    """Track plus verified (green) and partial-credit (blue) fills scaled to `width`."""
    solid = float(Fraction(verified) * Fraction(width) / 100)
    total = float(Fraction(done) * Fraction(width) / 100)
    parts = [f'<rect x="{x:.2f}" y="{y}" width="{width:.2f}" height="{height}" rx="{radius}" fill="{TRACK}"/>']
    if total > solid:
        parts.append(f'<rect x="{x:.2f}" y="{y}" width="{max(total, 2):.2f}" height="{height}" rx="{radius}" fill="{BLUE}"/>')
    if solid > 0:
        parts.append(f'<rect x="{x:.2f}" y="{y}" width="{max(solid, 2):.2f}" height="{height}" rx="{radius}" fill="{GREEN}"/>')
    return parts


def text(x: float, y: float, value: str, size: int = 12, fill: str = SECONDARY, anchor: str = "start", weight: str = "400") -> str:
    return (f'<text x="{x:.2f}" y="{y}" fill="{fill}" font-size="{size}"'
            + (f' text-anchor="{anchor}"' if anchor != "start" else "")
            + (f' font-weight="{weight}"' if weight != "400" else "") + f'>{value}</text>')


def render_svg(data: dict) -> str:
    left, width = 24, 952
    milestone_rows = (len(data["milestones"]) + 1) // 2
    scope_y = 150
    milestone_y = scope_y + 92
    table_y = milestone_y + 40 + milestone_rows * 30 + 12
    row = 30
    legend_y = table_y + 34 + len(data["groups"]) * row + 18
    height = legend_y + 76
    lines = [f'<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="{height}" viewBox="0 0 1000 {height}" role="img" aria-labelledby="title desc">',
             f'<title id="title">Rustic engine completion: {data["percent"]}%</title>',
             f'<desc id="desc">Weighted engine completion is {data["percent"]}%: {data["verified_percent"]}% from capabilities verified complete and {data["partial_percent"]}% partial credit from verified steps of in-progress capabilities. '
             'A scope map sizes every subsystem by its engine weight, milestone bars follow the roadmap, and each subsystem row shows capability cells sized by relative scope. Green is verified complete, blue is verified partial progress, gray is remaining.</desc>',
             f'<rect width="1000" height="{height}" rx="12" fill="#0d1117"/>',
             '<g font-family="DejaVu Sans,Arial,sans-serif">',
             text(left, 38, "Rustic · engine progress", 22, INK, weight="700"),
             text(left, 60, f'{data["completed"]}/{data["total"]} capabilities complete · {data["in_progress"]} in progress · {len(data["groups"])} subsystems · {escape(data["catalog_version"])}', 12, MUTED),
             text(976, 42, f'{data["percent"]}%', 32, INK, "end", "700"),
             text(976, 60, "weighted engine completion", 11, MUTED, "end")]
    lines += bar(left, 76, width, 14, data["percent"], data["verified_percent"], 4)
    remaining = percent(100 - Fraction(data["percent"]))
    lines += [f'<rect x="{left}" y="104" width="10" height="10" rx="2" fill="{GREEN}"/>',
              text(left + 16, 113, f'{data["verified_percent"]}% verified complete', 12),
              f'<rect x="{left + 200}" y="104" width="10" height="10" rx="2" fill="{BLUE}"/>',
              text(left + 216, 113, f'+{data["partial_percent"]}% partial credit (verified steps)', 12),
              f'<rect x="{left + 500}" y="104" width="10" height="10" rx="2" fill="{EMPTY}"/>',
              text(left + 516, 113, f'{remaining}% remaining', 12)]
    # Scope map: each block's width is its share of the engine, so filled area equals overall completion.
    lines += [text(left, scope_y - 8, "Scope map", 14, INK, weight="600"),
              text(976, scope_y - 8, "block width = share of engine · fill = subsystem progress", 11, MUTED, "end")]
    x = float(left)
    for index, group in enumerate(data["groups"]):
        span = width * group["weight"] / 100
        label = escape(group["label"])
        lines.append(f'<g><title>{label}: {group["percent"]}% of {group["weight"]}% engine weight ({group["contribution"]} engine points)</title>')
        lines += bar(x + 1, scope_y, span - 2, 32, group["percent"], group["verified_percent"], 3)
        # Alternate label rows so narrow neighbouring blocks never collide.
        lines += [f'<rect x="{x + span / 2 - 0.5:.2f}" y="{scope_y + 34}" width="1" height="{4 + 12 * (index % 2)}" fill="{LINE}"/>',
                  text(x + span / 2, scope_y + 48 + 12 * (index % 2), SHORT[group["id"]], 10, MUTED, "middle")]
        lines.append('</g>')
        x += span
    # Milestones from docs/ROADMAP.md, weighted by the engine share of their capabilities.
    lines += [text(left, milestone_y + 4, "Roadmap milestones", 14, INK, weight="600"),
              text(976, milestone_y + 4, "share = portion of the whole engine scheduled in that milestone", 11, MUTED, "end")]
    for index, milestone in enumerate(data["milestones"]):
        column, line = divmod(index, milestone_rows)
        mx, my = left + column * 488, milestone_y + 24 + line * 30
        label = escape(milestone["label"])
        lines += [f'<g><title>{milestone["id"]} {label}: {milestone["percent"]}% ({milestone["completed"]}/{milestone["total"]} capabilities, {milestone["share"]}% of engine)</title>',
                  text(mx, my + 12, milestone["id"], 12, MUTED, weight="600"),
                  text(mx + 36, my + 12, label, 12, SECONDARY)]
        lines += bar(mx + 200, my + 3, 150, 10, milestone["percent"], milestone["verified_percent"])
        lines += [text(mx + 410, my + 12, f'{milestone["percent"]}%', 12, INK, "end", "600"),
                  text(mx + 464, my + 12, f'{milestone["share"]}%', 11, MUTED, "end"), '</g>']
    # Subsystem table.
    lines += [text(left, table_y, "Subsystems", 14, INK, weight="600"),
              text(left, table_y + 22, "Subsystem", 11, MUTED), text(236, table_y + 22, "Weight", 11, MUTED, "end"),
              text(252, table_y + 22, "Progress", 11, MUTED), text(504, table_y + 22, "Done", 11, MUTED, "end"),
              text(520, table_y + 22, "Capabilities (width = relative scope)", 11, MUTED),
              text(976, table_y + 22, "Points", 11, MUTED, "end"),
              f'<rect x="{left}" y="{table_y + 29}" width="{width}" height="1" fill="{LINE}"/>']
    for index, group in enumerate(data["groups"]):
        y = table_y + 34 + index * row
        label = escape(group["label"])
        lines.append(f'<g><title>{label}: {group["completed"]}/{group["total"]} capabilities complete, {group["percent"]}% done, {group["contribution"]} of {group["weight"]} engine points</title>')
        if index % 2:
            lines.append(f'<rect x="{left}" y="{y}" width="{width}" height="{row}" fill="{CARD}"/>')
        lines += [text(left + 8, y + 19, label, 12, INK), text(236, y + 19, f'{group["weight"]}%', 12, MUTED, "end")]
        lines += bar(252, y + 10, 200, 10, group["percent"], group["verified_percent"])
        lines.append(text(504, y + 19, f'{group["percent"]}%', 12, INK, "end", "600"))
        cx, scale = 520.0, Fraction(380, group["points"])
        for feature in group["features"]:
            cell = float(scale * feature["size"])
            status = feature["status"].replace("_", " ")
            tip = f'{escape(feature["label"])}: {status}, size {feature["size"]}, {feature["milestone"]}'
            if feature["status"] == "in_progress":
                tip += f', {feature["credit"]}% verified'
            lines.append(f'<g><title>{tip}</title>')
            if feature["status"] == "complete":
                lines.append(f'<rect x="{cx:.2f}" y="{y + 8}" width="{cell - 2:.2f}" height="14" rx="2" fill="{GREEN}"/>')
            else:
                lines.append(f'<rect x="{cx:.2f}" y="{y + 8}" width="{cell - 2:.2f}" height="14" rx="2" fill="{EMPTY}"/>')
                filled = float(Fraction(feature["credit"]) * Fraction(cell - 2) / 100)
                if filled:
                    lines.append(f'<rect x="{cx:.2f}" y="{y + 8}" width="{filled:.2f}" height="14" rx="2" fill="{BLUE}"/>')
                if feature["status"] == "in_progress":
                    lines.append(f'<rect x="{cx + 0.5:.2f}" y="{y + 8.5}" width="{cell - 3:.2f}" height="13" rx="2" fill="none" stroke="{BLUE}"/>')
            lines.append('</g>')
            cx += cell
        lines += [text(976, y + 19, f'{group["contribution"]}', 12, SECONDARY, "end"), '</g>']
    for x, color, label, outline in [(left, GREEN, "Verified complete", False), (190, BLUE, "Partial credit from verified steps", False),
                                     (450, EMPTY, "In progress (outlined)", True), (640, EMPTY, "Planned / remaining", False)]:
        lines.append(f'<rect x="{x}" y="{legend_y}" width="12" height="12" rx="2" fill="{color}"' + (f' stroke="{BLUE}"' if outline else "") + '/>')
        lines.append(text(x + 20, legend_y + 11, label, 12))
    lines += [text(left, legend_y + 38, "Points: a subsystem's weight × its done fraction; all points sum to the overall percentage. 100% requires every capability and final acceptance on all five platforms.", 11, SECONDARY),
              text(left, legend_y + 58, f'Weighted delivery progress · not a web-compatibility percentage · source snapshot {data["source_fingerprint"][:12]}', 11, MUTED),
              '</g>', '</svg>']
    return "\n".join(lines) + "\n"


def summary(data: dict) -> str:
    lines = ["# Engine progress snapshot", "",
             f"**Overall completion: {data['percent']}%** = {data['verified_percent']}% verified complete + {data['partial_percent']}% partial credit. "
             f"{data['completed']}/{data['total']} capabilities complete, {data['in_progress']} in progress.", "",
             f"Catalog: `{data['catalog_version']}`. Source fingerprint: `{data['source_fingerprint']}`.", "",
             "100% requires all capabilities and final five-platform engine acceptance. Weights and sizes measure planned engine scope; they are not compatibility results or time estimates. See [PROGRESS.md](../PROGRESS.md).", "",
             "## Milestones", "", "| Milestone | Capabilities complete | Progress | Verified | Share of engine |", "| --- | --- | --- | --- | --- |"]
    for milestone in data["milestones"]:
        lines.append(f"| {milestone['id']} {milestone['label']} | {milestone['completed']}/{milestone['total']} | {milestone['percent']}% | {milestone['verified_percent']}% | {milestone['share']}% |")
    lines += ["", "## Subsystems", "", "| Subsystem | Completed | In progress | Progress | Verified | Engine weight | Engine points |", "| --- | --- | --- | --- | --- | --- | --- |"]
    for group in data["groups"]:
        lines.append(f"| {group['label']} | {group['completed']}/{group['total']} | {group['in_progress']} | {group['percent']}% | {group['verified_percent']}% | {group['weight']}% | {group['contribution']} |")
    lines += ["", "## Capability evidence", "", "Each line shows status, scope size (1, 2, 3, 5 or 8 points within its subsystem), roadmap milestone and share of the whole engine.", ""]
    for group in data["groups"]:
        lines.extend([f"### {group['label']}", ""])
        for feature in group["features"]:
            proof = ", ".join(f"[{entry['kind']}](../../{entry['path']})" for entry in feature["evidence"])
            status = feature["status"].replace("_", " ")
            if feature["status"] == "in_progress":
                status += f" {feature['credit']}%"
            lines.append(f"- **{status}** `{feature['id']}` — {feature['label']} · size {feature['size']} · {feature['milestone']} · {feature['engine_share']}% of engine"
                         + (f" ({proof})" if proof else ""))
            for step in feature.get("steps", []):
                mark = "x" if step["status"] == "complete" else " "
                proof = ", ".join(f"[{entry['kind']}](../../{entry['path']})" for entry in step.get("evidence", []))
                lines.append(f"  - [{mark}] {step['label']}" + (f" ({proof})" if proof else ""))
        lines.append("")
    return "\n".join(lines)


def readme_block(data: dict) -> str:
    current = next((milestone for milestone in data["milestones"] if milestone["completed"] < milestone["total"]), None)
    focus = f" Current milestone: **{current['id']} {current['label']}**, {current['percent']}% done." if current else ""
    return "\n".join([START, "", "## Engine progress", "",
        "[![Rustic engine progress map](docs/progress/engine.svg)](docs/progress/summary.md)", "",
        f"**Overall engine completion: {data['percent']}%** ({data['verified_percent']}% verified complete + {data['partial_percent']}% partial credit) — "
        f"{data['completed']}/{data['total']} capabilities complete and {data['in_progress']} in progress across {len(data['groups'])} subsystems.{focus}", "",
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
