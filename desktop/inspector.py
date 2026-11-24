"""Native navigation over the tutor's read-only journal projections."""
import datetime
import json
import os
import subprocess
import threading

from gi.repository import GLib, Gtk, Pango


def label(text, style=None, selectable=False):
    widget = Gtk.Label(label=text, xalign=0, selectable=selectable, wrap=True,
                       wrap_mode=Pango.WrapMode.WORD_CHAR)
    if style:
        widget.add_css_class(style)
    return widget


FILTERS = {
    "Lesson events": None,
    "Teaching and evidence": {"delivered", "continued", "recovered", "supervisor_plan", "discovery_feedback"},
    "Model processing": {"turn_step", "review", "rejected", "failed"},
    "Timing": {"timing"},
    "Saved drafts": {"workspace_draft"},
}


EVIDENCE_TYPES = {
    "independent": "Independent work", "cued": "Work after a cue", "reported": "Self-report",
    "correction": "Learner correction", "unfamiliar": "Reported unfamiliarity",
    "declined": "Declined task", "difficulty": "Observed difficulty",
}
BATCH = 96


class Inspector(Gtk.Box):
    def __init__(self):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        self.add_css_class("rail")
        self.add_css_class("explorer")
        self.path = None
        self.generation = self.offset = 0
        self.observed_size = -1
        self.identity = None
        self.busy = self.updating = False
        self.records = []
        self.timer = None
        self.header = Gtk.Box(spacing=6)
        self.append(self.header)
        self.title = label("Backend explorer", "section-title")
        self.title.set_hexpand(True)
        self.header.append(self.title)
        self.follow = Gtk.CheckButton(label="Follow latest", active=True)
        self.header.append(self.follow)
        self.status = label("", "muted")
        self.append(self.status)
        self.status.set_ellipsize(Pango.EllipsizeMode.MIDDLE)
        self.status.connect("notify::label", lambda widget, _: widget.set_tooltip_text(widget.get_text()))
        self.nav = Gtk.Box(spacing=6)
        self.append(self.nav)
        self.filter = Gtk.ComboBoxText()
        for name in FILTERS:
            self.filter.append_text(name)
        self.filter.set_active(0)
        self.filter.update_property([Gtk.AccessibleProperty.LABEL], ["Event category"])
        self.nav.append(self.filter)
        self.picker = Gtk.ComboBoxText(hexpand=True)
        self.picker.update_property([Gtk.AccessibleProperty.LABEL], ["Journal record"])
        self.nav.append(self.picker)
        self.scroll = Gtk.ScrolledWindow(vexpand=True, hscrollbar_policy=Gtk.PolicyType.NEVER)
        self.append(self.scroll)
        self.details = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        self.scroll_body = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        self.scroll_body.append(self.details)
        self.scroll.set_child(self.scroll_body)
        self.filter.connect("changed", lambda *_: self.populate())
        self.picker.connect("changed", self.selected)
        self.follow.connect("toggled", lambda b: self.populate() if b.get_active() else None)
        self.connect("map", self.mapped)
        self.connect("unmap", self.unmapped)

    def set_compact(self, compact):
        if compact == self.has_css_class("compact"):
            return
        if compact:
            self.add_css_class("compact")
            self.remove(self.status)
            self.header.prepend(self.status)
            # In the short lower pane the selectors scroll with the record,
            # leaving the whole viewport available for reading its fields.
            self.remove(self.nav)
            self.scroll_body.prepend(self.nav)
        else:
            self.remove_css_class("compact")
            self.header.remove(self.status)
            self.insert_child_after(self.status, self.header)
            self.scroll_body.remove(self.nav)
            self.insert_child_after(self.nav, self.status)
        self.title.set_visible(not compact)
        self.status.set_hexpand(compact)

    def bind(self, path):
        if path == self.path:
            return
        self.reset(path)

    def reset(self, path):
        self.generation += 1
        self.path, self.offset, self.observed_size = path, 0, -1
        self.busy = False
        self.records.clear()
        self.follow.set_active(True)
        self.filter.set_active(0)
        self.status.set_text("Reading the lesson journal…")
        self.populate()

    def mapped(self, *_):
        self.refresh()
        self.timer = GLib.timeout_add_seconds(1, self.refresh)

    def unmapped(self, *_):
        if self.timer:
            GLib.source_remove(self.timer)
            self.timer = None

    def refresh(self):
        if not self.path or self.busy:
            return True
        try:
            stat = self.path.stat()
            size = stat.st_size
            identity = (stat.st_dev, stat.st_ino)
            if self.identity != identity or size < self.offset:
                self.reset(self.path)
                self.identity = identity
        except OSError:
            self.status.set_text("Waiting for the lesson journal.")
            return True
        if size == self.observed_size:
            return True
        path, generation, offset = self.path, self.generation, self.offset
        self.busy = True

        def read():
            try:
                run = subprocess.run([os.environ["ROBOTUTOR_BIN"], "--inspect-journal", path, str(offset), str(BATCH)],
                                     capture_output=True, text=True, encoding="utf-8", timeout=30)
                rows = [json.loads(line) for line in run.stdout.splitlines()]
                error = run.stderr.strip() if run.returncode else ""
            except (OSError, ValueError, subprocess.TimeoutExpired):
                rows, error = [], "Could not read the journal. Retrying."
            GLib.idle_add(self.received, generation, size, rows, error)

        threading.Thread(target=read, daemon=True).start()
        return True

    def received(self, generation, size, rows, error):
        if generation != self.generation:
            return False
        self.busy = False
        self.observed_size = -1 if error else size
        if len(rows) == BATCH and rows[-1]["next_offset"] < size:
            self.observed_size = -1
        if rows:
            first = len(self.records)
            self.records.extend(r["record"] for r in rows)
            self.offset = rows[-1]["next_offset"]
            self.populate(first)
        self.status.set_text(error or f"{self.path.name}  ·  {len(self.records)} committed records")
        return False

    def populate(self, first=0):
        old = self.picker.get_active_id()
        self.updating = True
        if first == 0:
            self.picker.remove_all()
        group = FILTERS[self.filter.get_active_text()]
        last = None
        for index in range(first, len(self.records)):
            record = self.records[index]
            kind = record["event"]
            if (group is None and kind != "workspace_draft") or (group and kind in group):
                last = str(index)
                self.picker.append(last, f"#{index + 1}   {record['title']}")
        if last is not None and (not old or self.follow.get_active()):
            self.picker.set_active_id(last)
        elif old and not self.picker.set_active_id(old):
            self.picker.set_active(0)
        self.updating = False
        if first == 0 or self.picker.get_active_id() != old:
            self.render()

    def selected(self, *_):
        if not self.updating:
            self.follow.set_active(False)
            self.render()

    def render(self):
        while child := self.details.get_first_child():
            self.details.remove(child)
        self.scroll.get_vadjustment().set_value(0)
        index = self.picker.get_active_id()
        if index is None:
            self.details.append(label("No records in this category.", "muted"))
            return
        record = self.records[int(index)]
        heading = Gtk.Box(spacing=10)
        title = label(record["title"], "section-title")
        title.set_hexpand(True)
        heading.append(title)
        copy = Gtk.Button(label="Copy record")
        copy.connect("clicked", lambda *_: self.get_clipboard().set(json.dumps(record, indent=2, ensure_ascii=False)))
        heading.append(copy)
        self.details.append(heading)
        if record["recorded_at"] is not None:
            try:
                stamp = datetime.datetime.fromtimestamp(record["recorded_at"]).astimezone().strftime("%b %d, %Y · %H:%M:%S %Z")
            except (ValueError, OSError, OverflowError):
                stamp = str(record["recorded_at"])
            self.details.append(label(stamp, "muted"))
        if not record["objects"]:
            self.details.append(label("This record has no inspectable detail.", "muted"))
        longest = max((o.get("micros", 0) for o in record["objects"]), default=0)
        by_id = {o["id"]: o for o in record["objects"] if "id" in o}
        self.evidence = {}
        for obj in record["objects"]:
            card = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
            card.add_css_class("card")
            self.details.append(card)
            fields = dict(obj["fields"])
            content = card
            if "id" in obj:
                kind = EVIDENCE_TYPES.get(fields.get("Evidence type"), fields.get("Evidence type", "Evidence"))
                expander = Gtk.Expander(label=f"{kind}   ·   {obj['id']}", expanded=obj["name"] == "Cited learner evidence")
                card.append(expander)
                self.evidence[obj["id"]] = expander
                content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6, margin_top=6)
                expander.set_child(content)
            else:
                content.append(label(obj["name"], "note-heading"))
            if "micros" in obj:
                ms = obj["micros"] / 1000
                duration = f"{ms:,.2f} ms" if obj["micros"] >= 1000 else f"{obj['micros']} µs"
                content.append(label(duration, "muted"))
                bar = Gtk.ProgressBar(fraction=obj["micros"] / longest if longest else 0)
                bar.add_css_class("stage-time")
                bar.update_property([Gtk.AccessibleProperty.LABEL], [f"{obj['name']} duration {duration}"])
                bar.set_tooltip_text("Bars are relative to the longest stage in this measurement.")
                content.append(bar)
            grid = Gtk.Grid(column_spacing=14, row_spacing=5)
            content.append(grid)
            for n, (key, value) in enumerate(obj["fields"]):
                if "id" in obj and key in ("Source", "Evidence type"):
                    continue
                field = label(key, "muted")
                field.set_max_width_chars(16)
                field.set_valign(Gtk.Align.START)
                grid.attach(field, 0, n, 1, 1)
                text = label(value, selectable=True)
                text.set_hexpand(True)
                grid.attach(text, 1, n, 1, 1)
            for relation in dict.fromkeys(k for k, _ in obj.get("links", [])):
                content.append(label(relation, "note-heading"))
                links = Gtk.FlowBox(selection_mode=Gtk.SelectionMode.NONE, column_spacing=5,
                                    min_children_per_line=1, max_children_per_line=3, homogeneous=False, halign=Gtk.Align.START)
                content.append(links)
                for key, target in obj["links"]:
                    if key != relation:
                        continue
                    if target not in by_id:
                        links.insert(label(target, selectable=True), -1)
                        continue
                    kind = dict(by_id[target]["fields"]).get("Evidence type", "Evidence")
                    button = Gtk.Button(label=f"{EVIDENCE_TYPES.get(kind, kind)}  ·  {target}", halign=Gtk.Align.START)
                    button.add_css_class("trace-link")
                    button.connect("clicked", lambda _, t=target: self.jump(t))
                    links.insert(button, -1)

    def jump(self, source):
        item = self.evidence.get(source)
        if item:
            item.set_expanded(True)
            item.grab_focus()
            # An offscreen expander may receive no frame callbacks. Scroll from the
            # viewport after expansion, even when the target itself is clipped.
            def reveal():
                if item.get_root() and item.get_mapped():
                    ok, rect = item.compute_bounds(self.scroll_body)
                    if ok:
                        self.scroll.get_vadjustment().set_value(rect.get_y())
                return False
            reveal()
            GLib.idle_add(reveal)
