"""Native planning, drafting and coaching view for private assignments."""
import sqlite3
from gi.repository import Gtk
from sustained import AssignmentStore, COACHES, FIELDS, LABELS, brief_text, planning_step, reason_for_assignment


class AssignmentWindow(Gtk.Window):
    def __init__(self, parent, folder, lesson_topic=""):
        super().__init__(title="Assignment", transient_for=parent, modal=True,
                         destroy_with_parent=True, default_width=610, default_height=620)
        self.add_css_class("robotutor")
        self.lesson_topic = lesson_topic
        self.store = AssignmentStore(folder)
        self.key = self.store.open()
        self.render()

    def label(self, box, value, style=None):
        label = Gtk.Label(label=value, xalign=0, wrap=True, selectable=style not in {"page-title", "section-title"})
        if style:
            label.add_css_class(style)
        box.append(label)
        return label

    def buttons(self, box, items):
        row = Gtk.FlowBox(selection_mode=Gtk.SelectionMode.NONE, homogeneous=False,
                          column_spacing=6, row_spacing=4, max_children_per_line=4)
        box.append(row)
        for title, callback in items:
            button = Gtk.Button(label=title)
            button.connect("clicked", lambda _, fn=callback: fn())
            row.append(button)
        return row

    def input(self, box, action, caption, choices=()):
        entry = Gtk.Entry(placeholder_text=caption, hexpand=True)
        row = Gtk.Box(spacing=6)
        submit = Gtk.Button(label="Send", sensitive=False)
        submit.add_css_class("suggested-action")
        entry.connect("changed", lambda e: submit.set_sensitive(bool(e.get_text().strip())))
        send = lambda *_: self.act(action, entry.get_text()) if entry.get_text().strip() else None
        submit.connect("clicked", send)
        entry.connect("activate", send)
        row.append(entry)
        row.append(submit)
        if choices:
            self.buttons(box, [(s, lambda v=s: self.act(action, v)) for s in choices])
        box.append(row)
        if action in {"answer", "reply"}:
            self.set_focus(entry)

    def act(self, action, answer=""):
        try:
            self.store.turn(self.record, action, answer)
            self.render()
        except (ValueError, sqlite3.Error) as exc:
            self.error.set_text(str(exc))

    def select(self, key):
        self.key = key
        self.render()

    def render(self):
        r = self.record = self.store.read(self.key)
        brief = r["brief"]
        outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8,
                        margin_start=18, margin_end=18, margin_top=14, margin_bottom=14)
        scroll = Gtk.ScrolledWindow(vexpand=True, hscrollbar_policy=Gtk.PolicyType.NEVER)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=9)
        scroll.set_child(box)
        outer.append(scroll)
        self.error = self.label(outer, "")
        self.error.connect("notify::label", lambda label, _: label.set_visible(bool(label.get_text())))
        self.error.set_visible(False)
        self.set_child(outer)
        planning = r["status"] == "planning"
        working = r["status"] == "working"
        self.label(box, "Plan an assignment" if planning else brief["topic"], "page-title")
        if planning:
            question, choices = planning_step(brief)
            if self.lesson_topic and brief.get("purpose") and not brief.get("topic"):
                choices = (self.lesson_topic,)
            if brief:
                self.label(box, brief_text(brief), "muted")
            if len(brief) == len(FIELDS):
                self.label(box, reason_for_assignment(brief))
            self.label(box, question, "section-title")
            if len(brief) < len(FIELDS):
                self.input(box, "answer", "Your own answer…", choices)
                if brief:
                    self.buttons(box, [("Back one step", lambda: self.act("back"))])
            else:
                start = self.buttons(box, [("Start this assignment", lambda: self.act("accept"))])
                accept = start.get_first_child().get_child()
                accept.add_css_class("suggested-action")
                self.set_focus(accept)
                edit = Gtk.Expander(label="Edit the plan")
                fields = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
                self.buttons(fields, [(label, lambda k=k: self.act("revise", k))
                                      for k, label in zip(FIELDS, LABELS)])
                edit.set_child(fields)
                box.append(edit)
        else:
            panel = Gtk.Expander(label="Assignment brief", expanded=not working)
            panel.set_child(Gtk.Label(label=brief_text(brief), xalign=0, wrap=True, selectable=True))
            box.append(panel)
            self.label(box, ("Current checkpoint · " + r["checkpoint"]) if working else "Finished assignment")
            self.label(box, "Your draft", "section-title")
            editor = Gtk.TextView(wrap_mode=Gtk.WrapMode.WORD_CHAR, editable=working,
                                  left_margin=10, right_margin=10, top_margin=8, bottom_margin=8)
            editor.update_property([Gtk.AccessibleProperty.LABEL], ["Assignment draft"])
            draft_scroll = Gtk.ScrolledWindow(min_content_height=130, has_frame=True)
            draft_scroll.set_child(editor)
            box.append(draft_scroll)
            editor.get_buffer().set_text(r["draft"])
            if working:
                if not r["coach"]:
                    self.set_focus(editor)
                saved = self.label(box, "Draft saves on this device as you type.", "muted")
                def changed(buf):
                    after = buf.get_text(buf.get_start_iter(), buf.get_end_iter(), True)
                    try:
                        self.store.save_draft(self.key, r["draft"], after)
                        r["draft"] = after
                        saved.set_text("Draft saved on this device.")
                        self.finish_button.set_sensitive(bool(after.strip()))
                    except (ValueError, sqlite3.Error) as exc:
                        self.error.set_text(str(exc))
                        # Keep the unsaved contents visible and selectable for recovery.
                        saved.set_text("Unsaved changes")
                        editor.set_editable(False)
                        self.work_actions.set_sensitive(False)
                editor.get_buffer().connect("changed", changed)
                self.work_actions = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
                box.append(self.work_actions)
                self.label(self.work_actions, "Assignment helper", "section-title")
                if r["turns"]:
                    self.label(self.work_actions, r["turns"][-1][1])
                if r["coach"]:
                    self.input(self.work_actions, "reply", "Reply to the helper…")
                self.buttons(self.work_actions, [(s, lambda v=s: self.act("coach", v)) for s in COACHES])
                checkpoint = Gtk.Expander(label="Set the next checkpoint")
                steps = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
                self.input(steps, "checkpoint", "One piece for your next sitting…")
                checkpoint.set_child(steps)
                self.work_actions.append(checkpoint)
                self.finish_button = Gtk.Button(label="Mark assignment finished", halign=Gtk.Align.START,
                                                sensitive=bool(r["draft"].strip()))
                self.finish_button.connect("clicked", lambda *_: self.act("finish"))
                self.work_actions.append(self.finish_button)
        if planning:
            self.label(box, "You can close this window and return to your plan.", "muted")
        if r["turns"]:
            history = Gtk.Expander(label="Previous turns")
            history.set_child(Gtk.Label(label="\n\n".join(f"You · {s}\nHelper · {h}" for s, h in r["turns"]),
                                        xalign=0, wrap=True, selectable=True))
            box.append(history)
        archived = [(k, topic) for k, topic in self.store.finished() if k != self.key]
        if archived:
            history = Gtk.Expander(label="Finished assignments")
            listbox = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
            for k, topic in archived:
                self.buttons(listbox, [(topic, lambda key=k: self.select(key))])
            history.set_child(listbox)
            box.append(history)
        if r["status"] == "finished":
            self.buttons(box, [("Open assignment planner", lambda: self.select(self.store.open()))])

