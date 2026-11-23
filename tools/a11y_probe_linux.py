#!/usr/bin/env python3
"""Read the accessibility tree of a running `rustic view` through AT-SPI.

The M8 `a11y.acceptance` probe for Linux (ADR 0073). It runs the window
under the caller's X server (Xvfb in CI), turns on the session's AT-SPI
bus the way a screen reader does, finds Rustic among the desktop's
applications and checks the roles, names and states a screen reader would
read for tests/a11y/probe.html. Run it inside `dbus-run-session` with
DISPLAY set and the system Python that has `gi` and the Atspi typelib:

    dbus-run-session -- /usr/bin/python3 tools/a11y_probe_linux.py target/debug/rustic

Exit status 0 means every expectation held; the tree it read is printed
either way.
"""

import os
import subprocess
import sys
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, Gio, GLib  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIXTURE = os.path.join(ROOT, "tests", "a11y", "probe.html")

# (role name as AT-SPI reports it, accessible name) pairs that must appear.
EXPECTED = [
    ("document frame", "Accessibility probe"),
    ("landmark", ""),
    ("heading", "Accessibility probe"),
    ("landmark", "Sections"),
    ("link", "First section"),
    ("push button", "Send"),
    ("image", "Company logo"),
    ("check box", "Remember me"),
]


def enable_accessibility() -> None:
    """Set org.a11y.Status.IsEnabled, as an assistive technology does."""
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    bus.call_sync(
        "org.a11y.Bus", "/org/a11y/bus", "org.freedesktop.DBus.Properties", "Set",
        GLib.Variant("(ssv)", ("org.a11y.Status", "IsEnabled", GLib.Variant("b", True))),
        None, Gio.DBusCallFlags.NONE, 5000, None)


def walk(node, depth, out, limit=2000):
    if node is None or len(out) >= limit:
        return
    try:
        role = node.get_role_name()
        name = node.get_name() or ""
        states = node.get_state_set()
        focusable = states.contains(Atspi.StateType.FOCUSABLE)
        checked = states.contains(Atspi.StateType.CHECKED)
        extents = node.get_extents(Atspi.CoordType.WINDOW) if node.get_component_iface() else None
    except GLib.Error:
        return
    size = (extents.width, extents.height) if extents is not None else (0, 0)
    out.append((depth, role, name, focusable, checked, size))
    for i in range(node.get_child_count()):
        walk(node.get_child_at_index(i), depth + 1, out, limit)


def find_rustic(pid):
    desktop = Atspi.get_desktop(0)
    for i in range(desktop.get_child_count()):
        app = desktop.get_child_at_index(i)
        try:
            if app is not None and app.get_process_id() == pid:
                return app
        except GLib.Error:
            continue
    return None


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: a11y_probe_linux.py RUSTIC_BINARY", file=sys.stderr)
        return 2
    enable_accessibility()
    Atspi.init()
    env = dict(os.environ, WAYLAND_DISPLAY="")
    child = subprocess.Popen([sys.argv[1], "view", "--window", "800x600", FIXTURE], env=env)
    try:
        nodes = []
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            if child.poll() is not None:
                print(f"rustic view exited early with {child.returncode}", file=sys.stderr)
                return 1
            app = find_rustic(child.pid)
            nodes = []
            if app is not None:
                walk(app, 0, nodes)
            found = [(node[1], node[2]) for node in nodes]
            if all(item in found for item in EXPECTED):
                break
            time.sleep(0.5)
        for depth, role, name, focusable, checked, size in nodes:
            print("  " * depth + role + (f' "{name}"' if name else "")
                  + (" focusable" if focusable else "") + (" checked" if checked else "")
                  + f" {size[0]}x{size[1]}")
        found = [(node[1], node[2]) for node in nodes]
        missing = [item for item in EXPECTED if item not in found]
        if missing:
            print(f"missing from the AT-SPI tree: {missing}", file=sys.stderr)
            return 1
        focusables = {node[2] for node in nodes if node[3]}
        if not {"Send", "First section", "Remember me"} <= focusables:
            print(f"button, checkbox and link must be focusable, focusable: {focusables}", file=sys.stderr)
            return 1
        if not any(node[1] == "check box" and node[4] for node in nodes):
            print("the checkbox must report the checked state", file=sys.stderr)
            return 1
        heading = next(node for node in nodes if node[1] == "heading")
        if heading[5][0] <= 0 or heading[5][1] <= 0:
            print(f"the heading must have on-screen bounds, got {heading[5]}", file=sys.stderr)
            return 1
        print("AT-SPI probe passed")
        return 0
    finally:
        child.terminate()
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()


if __name__ == "__main__":
    sys.exit(main())
