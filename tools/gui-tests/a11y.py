#!/usr/bin/env python3
"""What accessibility (AT-SPI) shows of Arkha, for the GUI tests: works the
same on X11 and Wayland, where window titles can't be read otherwise.

    a11y.py windows     the titles of Arkha's windows, one per line
    a11y.py tabs        the tab names of its first window, in order
    a11y.py dialogs     its open dialogs' titles
    a11y.py listen S    keeps listening to focus events for S seconds, as
                        screen readers do (GTK then does more for them)
"""
import sys

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib  # noqa: E402


def arkha():
    desk = Atspi.get_desktop(0)
    for i in range(desk.get_child_count()):
        app = desk.get_child_at_index(i)
        if app and app.get_name() == "arkha":
            return app
    return None


def windows(app, roles=("frame",)):
    out = []
    for i in range(app.get_child_count()):
        w = app.get_child_at_index(i)
        if w and w.get_role_name() in roles:
            out.append(w)
    return out


def find(a, role, out, limit=4000, showing=True):
    """Everything of `role` under `a` (only shown ones if `showing`), depth first."""
    stack = [a]
    while stack and limit > 0:
        limit -= 1
        x = stack.pop()
        try:
            if x.get_role_name() == role and (not showing or x.get_state_set().contains(Atspi.StateType.SHOWING)):
                out.append(x)
            n = x.get_child_count()
            stack.extend(c for c in (x.get_child_at_index(k) for k in range(n - 1, -1, -1)) if c)
        except GLib.Error:
            pass
    return out


def main():
    Atspi.init()
    what = sys.argv[1] if len(sys.argv) > 1 else "windows"
    if what == "listen":
        listener = Atspi.EventListener.new(lambda e: None)
        for ev in ("object:active-descendant-changed", "object:state-changed", "focus:"):
            listener.register(ev)
        GLib.timeout_add_seconds(int(sys.argv[2]) if len(sys.argv) > 2 else 30, lambda: Atspi.event_quit())
        Atspi.event_main()
        return
    app = arkha()
    if not app:
        return
    if what == "windows":
        for w in windows(app):
            print(w.get_name())
    elif what == "tabs":
        ws = windows(app)
        if ws:
            # Tabs scrolled out of the tab bar too.
            for t in find(ws[0], "page tab", [], showing=False):
                print(t.get_name())
    elif what == "dialogs":
        for w in windows(app, ("dialog", "alert", "file chooser")):
            print(w.get_name())


main()
