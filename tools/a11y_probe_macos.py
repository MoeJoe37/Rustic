#!/usr/bin/env python3
"""Read the accessibility tree of a running `rustic view` through NSAccessibility.

The M8 `a11y.acceptance` probe for macOS (ADR 0073): the same check as
tools/a11y_probe_linux.py, made through the AX API that VoiceOver reads,
by way of System Events (JavaScript for Automation). The calling process
needs the Accessibility (TCC) grant; GitHub's macOS runners grant it to
their shell. Usage:

    python3 tools/a11y_probe_macos.py target/debug/rustic

Exit status 0 means every expectation held; the tree it read is printed
either way.
"""

import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIXTURE = os.path.join(ROOT, "tests", "a11y", "probe.html")

# (AXRole, accessible name) pairs that must appear. The name is the first
# non-empty of AXTitle, AXDescription and AXValue, as VoiceOver reads it.
EXPECTED = [
    ("Heading", "Accessibility probe"),
    ("AXLink", "First section"),
    ("AXButton", "Send"),
    ("AXCheckBox", "Remember me"),
    ("AXImage", "Company logo"),
]

SCRIPT = r"""
function run(argv) {
  const pid = parseInt(argv[0], 10);
  const se = Application('System Events');
  const procs = se.processes.whose({unixId: pid});
  if (procs.length === 0) return 'NOPROCESS';
  const proc = procs[0];
  if (proc.windows.length === 0) return 'NOWINDOW';
  const out = [];
  function attr(e, name) {
    try { const v = e.attributes.byName(name).value(); return v === null ? '' : String(v); }
    catch (x) { return ''; }
  }
  function walk(e, depth) {
    if (out.length > 2000) return;
    const role = attr(e, 'AXRole');
    const name = attr(e, 'AXTitle') || attr(e, 'AXDescription') || '';
    const value = attr(e, 'AXValue');
    const focusable = attr(e, 'AXFocused') !== '' ? 'f' : '';
    out.push([depth, role, name, value, focusable].join('\t'));
    let kids = [];
    try { kids = e.uiElements(); } catch (x) { kids = []; }
    for (let i = 0; i < kids.length; i++) walk(kids[i], depth + 1);
  }
  walk(proc.windows[0], 0);
  return out.join('\n');
}
"""


def read_tree(pid):
    result = subprocess.run(["osascript", "-l", "JavaScript", "-e", SCRIPT, str(pid)],
                            capture_output=True, text=True, timeout=60)
    if result.returncode != 0:
        return None, result.stderr.strip()
    lines = result.stdout.strip()
    if lines in ("NOPROCESS", "NOWINDOW"):
        return None, lines
    nodes = []
    for line in lines.splitlines():
        parts = line.split("\t")
        if len(parts) == 5:
            # AccessKit reports headings with the role string "Heading"
            # (WebKit's AXHeading); both are what VoiceOver announces.
            role = "Heading" if parts[1] == "AXHeading" else parts[1]
            nodes.append((int(parts[0]), role, parts[2], parts[3]))
    return nodes, ""


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: a11y_probe_macos.py RUSTIC_BINARY", file=sys.stderr)
        return 2
    child = subprocess.Popen([sys.argv[1], "view", "--window", "800x600", FIXTURE])
    try:
        nodes, why = [], ""
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            if child.poll() is not None:
                print(f"rustic view exited early with {child.returncode}", file=sys.stderr)
                return 1
            nodes, why = read_tree(child.pid)
            if nodes and all(item in {(n[1], n[2]) for n in nodes} for item in EXPECTED):
                break
            time.sleep(1)
        if not nodes:
            print(f"could not read the window through System Events: {why}", file=sys.stderr)
            return 1
        for depth, role, name, value in nodes:
            print("  " * depth + role + (f' "{name}"' if name else "")
                  + (f" value={value}" if value else ""))
        found = {(n[1], n[2]) for n in nodes}
        missing = [item for item in EXPECTED if item not in found]
        if missing:
            print(f"missing from the AX tree: {missing}", file=sys.stderr)
            return 1
        if not any(n[1] == "AXCheckBox" and n[3] in ("1", "true") for n in nodes):
            print("the checkbox must report AXValue 1", file=sys.stderr)
            return 1
        print("NSAccessibility probe passed")
        return 0
    finally:
        child.terminate()
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()


if __name__ == "__main__":
    sys.exit(main())
