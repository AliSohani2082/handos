#!/usr/bin/python3
"""Spike 3 GTK target: a GTK 4 entry that writes its text to argv[1] when Enter is pressed."""
import sys

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk  # noqa: E402


def on_activate(app):
    win = Gtk.ApplicationWindow(application=app, title="handOs spike GTK entry")
    entry = Gtk.Entry()

    def done(_):
        with open(sys.argv[1], "w", encoding="utf-8") as f:
            f.write(entry.get_text())
        app.quit()

    entry.connect("activate", done)
    win.set_child(entry)
    win.present()
    entry.grab_focus()


app = Gtk.Application(application_id="org.handos.SpikeEntry")
app.connect("activate", on_activate)
app.run([sys.argv[0]])
