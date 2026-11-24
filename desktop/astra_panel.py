"""Read and act on the current workspace's private Astra comparisons."""
import json
import sqlite3
import subprocess
import sys
from pathlib import Path

from gi.repository import GLib, Gtk

from astra import connect


class Reviews(Gtk.Box):
    def __init__(self):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        self.add_css_class("workspace-body")
        self.folder = None
        self.records = {}
        self.shown = None
        self.timer = None
        title = Gtk.Label(label="Astra reviews", xalign=0)
        title.add_css_class("page-title")
        self.append(title)
        self.summary = Gtk.Label(xalign=0, wrap=True)
        self.summary.add_css_class("muted")
        self.append(self.summary)
        self.picker = Gtk.ComboBoxText()
        self.picker.update_property([Gtk.AccessibleProperty.LABEL], ["Recorded comparison"])
        self.picker.connect("changed", lambda *_: self.select())
        self.append(self.picker)
        scroll = Gtk.ScrolledWindow(vexpand=True, hscrollbar_policy=Gtk.PolicyType.NEVER)
        self.append(scroll)
        self.details = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, valign=Gtk.Align.START)
        self.details.add_css_class("card")
        scroll.set_child(self.details)
        actions = Gtk.Box(spacing=8)
        self.append(actions)
        self.toggle = Gtk.Button(label="Mark reviewed")
        self.retry = Gtk.Button(label="Retry blocked review")
        actions.append(self.toggle)
        actions.append(self.retry)
        self.toggle.connect("clicked", lambda *_: self.change(False))
        self.retry.connect("clicked", lambda *_: self.change(True))
        self.connect("map", self.mapped)
        self.connect("unmap", self.unmapped)

    def mapped(self, *_):
        self.refresh()
        self.timer = GLib.timeout_add_seconds(3, self.refresh)

    def unmapped(self, *_):
        if self.timer:
            GLib.source_remove(self.timer)
            self.timer = None

    def bind(self, folder):
        if folder != self.folder:
            self.folder = folder
            self.records.clear()
            self.picker.remove_all()
        self.refresh()

    def line(self, text, style=None):
        label = Gtk.Label(label=text, xalign=0, wrap=True, selectable=True)
        if style:
            label.add_css_class(style)
        self.details.append(label)

    def select(self):
        row = self.records.get(self.picker.get_active_id())
        self.toggle.set_visible(bool(row and row["status"] == "note"))
        self.toggle.set_label("Reopen note" if row and row["resolved"] else "Mark reviewed")
        self.retry.set_visible(bool(row and row["status"] == "blocked"))
        self.picker.set_visible(bool(self.records))
        if row == self.shown and self.details.get_first_child():
            return
        self.shown = row
        while child := self.details.get_first_child():
            self.details.remove(child)
        if not row:
            self.line("No comparisons yet", "section-title")
            self.line("After Luna replies, Astra independently answers the same request and checks for a concrete improvement.")
            return
        self.line(f"Lesson {row['session']} · {row['stage']} · request {row['digest'][:12]}", "muted")
        if row["status"] == "note":
            analysis = json.loads(row["analysis"])
            for label, key in (("What Astra would improve", "observation"), ("Luna wrote", "luna_evidence"),
                               ("Astra wrote", "astra_evidence"), ("Proposed Luna change", "luna_change"),
                               ("How to test the change", "replay_check")):
                self.line(label, "note-heading")
                self.line(analysis[key], "review-quote" if key.endswith("_evidence") else None)
        elif row["status"] == "blocked":
            self.line(row["error"])
        elif row["status"] == "done":
            self.line("Astra found no material improvement on this exchange.")
        else:
            self.line("Astra is comparing the answers." if row["candidate_ready"]
                      else "Waiting for Astra’s independent answer.")

    def refresh(self):
        rows = []
        if self.folder and (self.folder / "reviews.sqlite3").is_file():
            try:
                with connect(self.folder) as db:
                    rows = db.execute("""SELECT id,session,stage,digest,status,resolved,error,analysis,
                                      candidate IS NOT NULL AS candidate_ready FROM reviews
                                      ORDER BY (status='note' AND resolved=0) DESC,id DESC""").fetchall()
            except sqlite3.Error:
                self.summary.set_text("Waiting for access to the private review database.")
                self.select()
                return True
        current = self.picker.get_active_id()
        updated = {str(row["id"]): dict(row) for row in rows}
        if self.records != updated:
            self.records = updated
            self.picker.remove_all()
            for key, row in updated.items():
                label = "reviewed" if row["resolved"] else row["status"]
                self.picker.append(key, f"#{key} · {row['session']} · {row['stage']} · {label}")
            if not current or not self.picker.set_active_id(current):
                self.picker.set_active(0)
        counts = (sum(r["status"] == "note" and not r["resolved"] for r in rows),
                  sum(r["status"] == "pending" for r in rows), sum(r["status"] == "blocked" for r in rows))
        notes = "note" if counts[0] == 1 else "notes"
        exchanges = "exchange" if len(rows) == 1 else "exchanges"
        self.summary.set_label(f"{counts[0]} open {notes} · {counts[1]} pending · {counts[2]} blocked · {len(rows)} {exchanges}")
        self.summary.set_visible(bool(rows))
        self.select()
        return True

    def change(self, retrying):
        key = self.picker.get_active_id()
        if not key:
            return
        with connect(self.folder) as db:
            if retrying:
                db.execute("UPDATE reviews SET status='pending',error='' WHERE id=? AND status='blocked'", (key,))
            else:
                db.execute("UPDATE reviews SET resolved=1-resolved WHERE id=? AND status='note'", (key,))
        if retrying:
            subprocess.Popen([sys.executable, str(Path(__file__).with_name("astra.py")), "wake", self.folder])
        self.refresh()
