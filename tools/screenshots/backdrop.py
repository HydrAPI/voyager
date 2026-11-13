#!/usr/bin/env python3
"""backdrop.py FLAG: a desktop window behind every other, as big as the
screen, for shots.sh: black while the file FLAG exists, else white, so that
each window can be taken on both and its transparency worked out
(alpha.sh)."""
import os
import sys

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402

flag = sys.argv[1]
w = Gtk.Window()
w.set_type_hint(Gdk.WindowTypeHint.DESKTOP)
w.set_decorated(False)
size = Gdk.Display.get_default().get_monitor(0).get_geometry()
w.set_default_size(size.width, size.height)
w.move(0, 0)
css = Gtk.CssProvider()
Gtk.StyleContext.add_provider_for_screen(w.get_screen(), css, Gtk.STYLE_PROVIDER_PRIORITY_USER)
shown = None


def update():
    global shown
    color = "#000000" if os.path.exists(flag) else "#ffffff"
    if color != shown:
        css.load_from_data(f"window {{ background: {color}; }}".encode())
        shown = color
    return True


update()
GLib.timeout_add(50, update)
w.show()
Gtk.main()
