#!/usr/bin/python3
"""GTK lesson workspace for the Rust tutor. Teacher submissions require explicit confirmation."""

import argparse
import datetime
import json
import os
import queue
import re
import signal
import stat
import subprocess
import sys
import threading
import time
import uuid
from pathlib import Path
from urllib.request import Request, urlopen

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, Gio, GLib, Gtk, Pango
from practice import ActivityEditor
from reading import ReadingView
from personal_courses import PersonalCourses
from courses import Courses
from inspector import Inspector
import astra_panel
from local_platform import background_options, private_directory, state_home
from test_learners import PRESETS, workspace as test_workspace

os.umask(0o077)
STATE = state_home() / "robotutor"
PERSONAL = Path(os.getenv("ROBOTUTOR_WORKSPACE", STATE / "learner-tests"))
CACHE = Path(os.getenv("ROBOTUTOR_ASSIGNMENTS", STATE / "assigned"))
URL = "http://127.0.0.1:" + str(int(os.getenv("ROBOTUTOR_PORT", "47321")))


def save(path, data):
    temp = path.with_suffix(".tmp-" + uuid.uuid4().hex)
    with temp.open("x") as stream:
        json.dump(data, stream)
        stream.flush()
        os.fsync(stream.fileno())
    temp.replace(path)


def api(action, **data):
    req = Request(
        URL + "/api",
        json.dumps(dict(action=action, **data)).encode(),
        {"Content-Type": "application/json", "X-Profile": "learner"},
    )
    with urlopen(req, timeout=1) as res:
        return json.load(res)


def words(view):
    buf = view.get_buffer()
    return buf.get_text(buf.get_start_iter(), buf.get_end_iter(), True)


def pane(widget, height=-1):
    scroll = Gtk.ScrolledWindow(min_content_height=height, hexpand=True, vexpand=True)
    scroll.set_child(widget)
    return scroll


def view(edit=False):
    return Gtk.TextView(
        editable=edit,
        wrap_mode=Gtk.WrapMode.WORD_CHAR,
        left_margin=10,
        right_margin=10,
        top_margin=8,
        bottom_margin=8,
    )


class Pilot(Gtk.Application):
    def __init__(self, assignment=None, register_desktop=False, test_learner=None):
        app_id = "org.robotutor.Pilot"
        super().__init__(
            application_id=app_id if register_desktop else None,
            flags=Gio.ApplicationFlags.HANDLES_COMMAND_LINE
            if register_desktop
            else Gio.ApplicationFlags.FLAGS_NONE,
        )
        self.test_learner = test_learner
        self.personal = PERSONAL
        self.cache = CACHE
        self.last_lesson = self.cache / "last-lesson.json"
        self.requested = assignment
        self.reset_requested = False
        self.restoring = False
        self.form = None
        self.revision = None
        self.sent_at = None
        self.proc = None
        self.epoch = 0
        self.output = queue.Queue()
        self.ready = False
        self.preview_drafts = {}
        self.assignment = None

    def configure_test(self, preset, reset=False):
        root = Path(os.getenv("ROBOTUTOR_TEST_HOME", STATE / "test-learners"))
        folder = test_workspace(root, preset, os.environ["ROBOTUTOR_BIN"], reset)
        self.personal, self.cache = folder / "personal", folder / "assigned"
        self.last_lesson = self.cache / "last-lesson.json"
        self.requested = "current.jsonl" if preset != "new" else None

    def restore_selection(self, assignment=None):
        private_directory(self.personal)
        private_directory(self.cache)
        if assignment is not None:
            self.requested = assignment
        elif self.last_lesson.is_file():
            try:
                self.requested = json.loads(self.last_lesson.read_text())
            except ValueError:
                pass

    def reset_test(self):
        if self.test_learner:
            self.switch_profile(self.test_learner, reset=True)

    def switch_profile(self, preset, reset=False, assignment=None):
        if preset != self.test_learner or reset:
            if preset:
                self.configure_test(preset, reset)
            else:
                self.personal, self.cache = PERSONAL, CACHE
                self.last_lesson = self.cache / "last-lesson.json"
                self.requested = None
            self.restore_selection(assignment)
            self.test_learner = preset
            self.stop()
            self.key = None
            self.updating = True
            self.picker.remove_all()
            self.updating = False
            self.refresh()
        elif assignment:
            self.requested = assignment
            self.refresh()
            if not self.picker.set_active_id(assignment):
                self.shared.set_text("Assignment unavailable on this workstation")
        self.profile_menu.set_label(PRESETS[preset][0] if preset else "My workspace")
        self.reset_button.set_visible(bool(preset))
        self.refresh_button.set_visible(not preset)
        self.nav["Courses"].set_tooltip_text("Open course management in My workspace")
        self.show_page("Lesson")

    def open_personal_activity(self, name):
        self.refresh()
        self.picker.set_active_id(name)
        self.show_page("Lesson")

    def open_assignment(self, key):
        self.switch_profile(None, assignment=key)

    def do_command_line(self, command):
        args = command.get_arguments()[1:]
        preset = assignment = None
        reset = False
        if args[:1] == ["--test"] and len(args) in (2, 3) and args[1] in PRESETS:
            preset = args[1]
            reset = args[2:] == ["--reset"]
            if len(args) == 3 and not reset:
                return 2
        elif args:
            match = re.fullmatch(r"robotutor://assignment/(\d+)", args[0])
            if len(args) != 1 or not match:
                return 2
            assignment = match[1]
        if hasattr(self, "win"):
            self.switch_profile(preset, reset, assignment)
        else:
            self.test_learner, self.requested, self.reset_requested = preset, assignment, reset
        self.activate()
        return 0

    def navigate(self, name):
        returning_from_courses = name == "Lesson" and self.pages.get_visible_child_name() == "Courses"
        self.show_page(name)
        if returning_from_courses:
            self.refresh()

    def show_page(self, name):
        if name == "Courses":
            self.private_courses.bind(self.personal)
            self.course_switch.set_visible(not self.test_learner)
            if self.test_learner:
                self.course_pages.set_visible_child_name("personal")
            elif self.course_pages.get_visible_child_name() == "class" and not self.courses.web:
                self.courses.load()
        if name == "Astra reviews":
            self.reviews.bind(self.folder / ".astra")
        self.pages.set_visible_child_name(name)
        if name in self.nav:
            self.nav[name].set_active(True)

    def do_activate(self):
        if hasattr(self, "win"):
            self.win.present()
            return
        if self.test_learner:
            self.configure_test(self.test_learner, reset=self.reset_requested)
        self.restore_selection(None if self.test_learner else self.requested)
        self.win = Gtk.ApplicationWindow(
            application=self, title="Robotutor", default_width=1160, default_height=740
        )
        self.win.add_css_class("robotutor")
        fonts = self.win.get_pango_context().get_font_map()
        if hasattr(fonts, "add_font_file"):
            fonts.add_font_file(str(Path(__file__).resolve().parents[1] / "web/brand/Nunito.ttf"))
        css = Gtk.CssProvider()
        css.load_from_path(str(Path(__file__).with_name("workspace.css")))
        Gtk.StyleContext.add_provider_for_display(self.win.get_display(), css, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        shell = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self.win.set_child(shell)
        mast = Gtk.HeaderBar()
        mast.add_css_class("mast")
        self.win.set_titlebar(mast)
        brand = Gtk.Box(spacing=8)
        logo = Gtk.Image.new_from_file(str(Path(__file__).resolve().parents[1] / "web/brand/robotutor-logo.svg"))
        logo.set_pixel_size(36)
        brand.append(logo)
        brand.append(self.heading("Robotutor", "brand"))
        mast.pack_start(brand)
        tabs = Gtk.Box(spacing=4)
        mast.set_title_widget(tabs)
        self.nav = {}
        for title in ("Lesson", "Courses", "Astra reviews"):
            btn = Gtk.ToggleButton(label=title)
            if self.nav:
                btn.set_group(self.nav["Lesson"])
            self.nav[title] = btn
            btn.connect("clicked", lambda _, t=title: self.navigate(t))
            tabs.append(btn)
        self.profile_menu = Gtk.MenuButton()
        mast.pack_end(self.profile_menu)
        menu = Gtk.Popover()
        choices = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        for key, (label, tip) in {None: ("My workspace", "Personal and assigned lessons"), **PRESETS}.items():
            self.button(choices, label, lambda k=key: (menu.popdown(), self.switch_profile(k))).set_tooltip_text(tip)
        self.reset_button = self.button(choices, "Reset test learner", lambda: (menu.popdown(), self.reset_test()))
        menu.set_child(choices)
        self.profile_menu.set_popover(menu)
        self.pages = Gtk.Stack(vexpand=True, hhomogeneous=False, vhomogeneous=False)
        shell.append(self.pages)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        box.add_css_class("workspace-body")
        self.pages.add_named(box, "Lesson")
        self.courses = Courses(URL, self.open_assignment)
        course_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        self.course_pages = Gtk.Stack(vexpand=True)
        self.course_switch = Gtk.StackSwitcher(stack=self.course_pages, halign=Gtk.Align.CENTER)
        self.private_courses = PersonalCourses(self.open_personal_activity)
        self.course_pages.add_titled(self.courses, "class", "Class courses")
        self.course_pages.add_titled(self.private_courses, "personal", "My private courses")
        self.course_pages.connect("notify::visible-child-name", lambda *_:
                                  self.courses.load() if self.course_pages.get_visible_child_name() == "class" and not self.courses.web else None)
        course_box.append(self.course_switch)
        course_box.append(self.course_pages)
        self.pages.add_named(course_box, "Courses")
        self.inspector = Inspector()
        self.reviews = astra_panel.Reviews()
        self.pages.add_named(self.reviews, "Astra reviews")
        bar = Gtk.Box(spacing=8)
        box.append(bar)
        self.lesson_title = self.heading("Your lesson", "page-title")
        self.lesson_title.set_hexpand(True)
        self.lesson_title.set_ellipsize(Pango.EllipsizeMode.END)
        bar.append(self.lesson_title)
        self.picker = Gtk.ComboBoxText()
        self.picker.update_property([Gtk.AccessibleProperty.LABEL], ["Select lesson"])
        bar.append(self.picker)
        self.refresh_button = self.button(bar, "Refresh", self.refresh)
        self.refresh_button.set_tooltip_text("Refresh assignments")
        self.notes_button = self.button(bar, "Notes", lambda: self.pages.set_visible_child_name("Notes"))
        self.notes_button.set_visible(False)
        self.explorer_button = Gtk.ToggleButton(label="Backend explorer")
        bar.append(self.explorer_button)
        self.context = Gtk.Label(xalign=0, wrap=True, selectable=True)
        split = Gtk.Paned(orientation=Gtk.Orientation.HORIZONTAL, position=770, vexpand=True,
                          shrink_start_child=False, shrink_end_child=False)
        box.append(split)
        left = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=7, margin_end=12)
        right = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        right.add_css_class("rail")
        split.set_start_child(left)
        rail_scroll = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER, min_content_width=240)
        rail_scroll.set_child(right)
        self.sidebar = Gtk.Paned(orientation=Gtk.Orientation.VERTICAL, shrink_start_child=False,
                                 shrink_end_child=False, position=210)
        self.sidebar.set_start_child(rail_scroll)
        self.sidebar.set_end_child(self.inspector)
        self.inspector.set_visible(False)
        split.set_end_child(self.sidebar)
        self.split, self.rail, self.narrow = split, rail_scroll, False
        self.compact_reason = Gtk.Label(xalign=0, ellipsize=Pango.EllipsizeMode.END, max_width_chars=60)
        self.compact_reason.add_css_class("muted")
        self.compact_reason.set_visible(False)
        left.append(self.compact_reason)
        right.append(self.heading("Lesson notes", "section-title"))
        right.append(self.context)
        right.append(Gtk.Separator())
        self.history = ReadingView()
        self.history_scroll = pane(self.history)
        self.history.add_css_class("reading")
        self.history_scroll.add_css_class("card")
        self.history_scroll.add_css_class("conversation")
        left.append(self.history_scroll)
        self.exercise = ReadingView()
        self.exercise.update_property([Gtk.AccessibleProperty.LABEL], ["Current question"])
        question_scroll = Gtk.ScrolledWindow(propagate_natural_height=True, max_content_height=1,
                                             hscrollbar_policy=Gtk.PolicyType.NEVER, has_frame=False)
        question_scroll.set_child(self.exercise)
        current = Gtk.Expander(label="Current question", expanded=True)
        current.add_css_class("current")
        current.set_child(question_scroll)
        left.append(current)
        self.question_panel, self.question_scroll = current, question_scroll
        self.quick_replies = Gtk.Box(spacing=6, visible=False)
        left.append(self.quick_replies)
        self.status = Gtk.Label(xalign=0, wrap=True)
        self.status.add_css_class("muted")
        self.activity = ActivityEditor(self.save_draft, self.command, self.import_document)
        self.activity_scroll = Gtk.ScrolledWindow(max_content_height=280, propagate_natural_height=True)
        self.activity_scroll.set_child(self.activity)
        left.append(self.activity_scroll)
        self.editor = view(True)
        self.editor.update_property([Gtk.AccessibleProperty.LABEL], ["Reply to Luna"])
        self.editor.set_accepts_tab(False)
        self.editor.get_buffer().connect("changed", self.answer_changed)
        self.composing = False
        self.editor.connect("preedit-changed", self.preedit_changed)
        keys = Gtk.EventControllerKey()
        keys.set_propagation_phase(Gtk.PropagationPhase.CAPTURE)
        keys.connect("key-pressed", self.answer_key)
        self.editor.add_controller(keys)

        composer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        composer.add_css_class("composer")
        left.append(composer)
        self.answer_scroll = Gtk.ScrolledWindow(
            min_content_height=62, max_content_height=150,
            propagate_natural_height=True, hexpand=True,
            hscrollbar_policy=Gtk.PolicyType.NEVER,
        )
        self.answer_scroll.set_child(self.editor)
        overlay = Gtk.Overlay()
        overlay.set_child(self.answer_scroll)
        self.answer_hint = Gtk.Label(
            label="Reply to Luna…", halign=Gtk.Align.START, valign=Gtk.Align.START,
            margin_start=10, margin_top=8, can_target=False,
        )
        self.answer_hint.add_css_class("dim-label")
        overlay.add_overlay(self.answer_hint)
        composer.append(overlay)
        footer = Gtk.Box(spacing=8, margin_start=10, margin_end=6, margin_bottom=6)
        shortcut = Gtk.Label(label="Enter to send · Shift+Enter for a new line", xalign=0,
                             hexpand=True, wrap=True)
        shortcut.add_css_class("caption")
        shortcut.add_css_class("dim-label")
        footer.append(shortcut)
        self.send_button = self.button(footer, "Send", self.send)
        self.send_button.add_css_class("suggested-action")
        self.send_button.set_sensitive(False)
        composer.append(footer)
        bar = Gtk.Box(spacing=6)
        left.append(bar)
        self.controls = [
            self.button(bar, label, fun)
            for label, fun in (
                ("Retry", lambda: self.command("/retry")),
                ("Plan", lambda: self.command("/plan")),
            )
        ]
        self.button(bar, "Assignment", self.open_assignment)
        self.actions = Gtk.MenuButton(label="Actions")
        bar.append(self.actions)
        popover = Gtk.Popover()
        actions = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=3)
        query = Gtk.SearchEntry(placeholder_text="Find a command")
        actions.append(query)
        self.action_buttons = []
        for label, cmd in (("Study plan", "/plan"), ("My skills", "/skills"), ("Teaching trace", "/trace"),
                           ("Correct an interpretation", "/amend "), ("Resume", "/continue"),
                           ("Graph", "/activity graph"), ("Build a table", "/activity table"),
                           ("Order steps", "/activity order"), ("Repair a formula", "/activity repair"),
                           ("More practice formats", "/activities")):
            def choose_action(c=cmd):
                popover.popdown()
                if c.endswith(" "):
                    self.editor.get_buffer().set_text(c)
                    self.editor.grab_focus()
                else:
                    self.command(c)
            self.action_buttons.append(self.button(actions, label, choose_action))
        query.connect("search-changed", lambda q: [b.set_visible(q.get_text().casefold() in b.get_label().casefold()) for b in self.action_buttons])
        popover.set_child(actions)
        self.actions.set_popover(popover)
        self.submit = self.button(bar, "Preview submission", self.preview)
        self.revise = self.button(bar, "Revise reviewed work", self.revise_work)
        bar.append(Gtk.Box(hexpand=True))
        bar.append(self.status)
        self.shared = Gtk.Label(xalign=0, wrap=True)
        self.shared.set_visible(False)
        self.shared.connect("notify::label", lambda label, _: label.set_visible(bool(label.get_text())))
        left.append(self.shared)
        self.reason = Gtk.Label(xalign=0, wrap=True, selectable=True)
        self.why = self.heading("Why this step", "note-heading")
        right.append(self.why)
        right.append(self.reason)
        def reason_changed(label, _):
            text = label.get_text()
            self.why.set_visible(bool(text))
            self.compact_reason.set_label(text)
            self.compact_reason.set_tooltip_text(text)
            self.compact_reason.set_visible(self.narrow and bool(text))
        self.reason.connect("notify::label", reason_changed)
        self.plan_view = view()
        self.plan_panel = Gtk.Expander(label="Study plan")
        self.plan_panel.set_child(pane(self.plan_view, 125))
        self.plan_panel.connect("notify::expanded", lambda panel, _: self.command("/plan")
                                if panel.get_expanded() and not words(self.plan_view) else None)
        self.corrections = Gtk.Box(spacing=4)
        right.append(self.corrections)
        self.corrections.set_orientation(Gtk.Orientation.VERTICAL)
        self.source = Gtk.ComboBoxText(hexpand=True)
        self.corrections.append(self.source)
        for cell in self.source.get_cells():
            cell.set_property("ellipsize", Pango.EllipsizeMode.END)
            cell.set_property("max-width-chars", 25)
        evidence_actions = Gtk.Box(spacing=6)
        self.corrections.append(evidence_actions)
        self.button(evidence_actions, "Show exchange", self.show_source)
        self.button(evidence_actions, "Correct this", self.correct_source)
        right.append(self.plan_panel)
        self.corrections.set_visible(False)
        self.explorer_button.connect("toggled", lambda *_: self.layout_lesson_panes())
        self.picker.connect("changed", self.choose)
        self.win.connect("close-request", lambda *_: self.stop() or False)
        self.refresh()
        self.switch_profile(self.test_learner)
        GLib.timeout_add(220, self.tick)
        self.win.connect(
            "notify::is-active",
            lambda w, _: self.refresh() if w.is_active() and self.assignment else None,
        )
        self.win.present()

    @staticmethod
    def heading(text, style):
        label = Gtk.Label(label=text, xalign=0)
        label.add_css_class(style)
        return label

    def button(self, parent, label, callback):
        b = Gtk.Button(label=label)
        b.connect("clicked", lambda *_: callback())
        parent.append(b)
        return b

    def refresh(self):
        prior = self.picker.get_active_id() or self.requested
        try:
            assignments = [] if self.test_learner else api("state")["assignments"]
            for a in assignments:
                key = str(int(a["id"]))
                folder = self.cache / key
                folder.mkdir(exist_ok=True, mode=0o700)
                save(folder / "assignment.json", a)
            online = True
        except (OSError, ValueError):
            online = False
        items = [
            (p.name, ({"current": "Current visit", "first-visit": "First visit", "workshop": "Workshop"}.get(p.stem, p.stem)
                      if self.test_learner else "Personal · " + p.stem))
            for p in sorted(
                self.personal.glob("*.jsonl"), key=lambda p: p.lstat().st_mtime, reverse=True
            )
        ]
        if not items:
            today = datetime.date.today().isoformat()
            items.append((today + ".jsonl", "Personal · " + today))
        for path in sorted(self.cache.glob("[0-9]*/assignment.json")):
            a = json.loads(path.read_text())
            items.append((str(a["id"]), a["activity"]["title"] + " · " + a["status"]))
        self.updating = True
        self.picker.remove_all()
        for key, label in items:
            self.picker.append(key, label)
        self.picker.set_active_id(prior if prior in dict(items) else items[0][0])
        self.updating = False
        if getattr(self, "key", None) != self.picker.get_active_id():
            self.choose(self.picker)
        elif self.assignment:
            self.load_assignment()
        if not online and not (self.assignment and self.outbox.exists()):
            self.shared.set_text("LMS offline · saved lessons available")

    def load_assignment(self):
        self.assignment = json.loads((self.folder / "assignment.json").read_text())
        a = self.assignment
        activity = a["activity"]
        self.context.set_text(
            f"{activity['title']} · {a['status']}\n{activity['instructions']}\nObjective: {activity['objective']}\nSubmit: {activity['criterion']}\n{a['feedback']}"
        )
        latest = a["submissions"][-1] if a["submissions"] else None
        if latest:
            self.context.set_text(self.context.get_text() + "\nReviewed version · " + latest["work"])
        self.revise.set_visible(bool(latest and a["status"] == "Revision requested"))
        self.pending_label()

    def set_sources(self, sources):
        selected = self.source.get_active_id()
        self.source.remove_all()
        for source, quote in sources:
            self.source.append(source, source + " · " + quote[:70])
        self.corrections.set_visible(bool(sources))
        if sources and (not selected or not self.source.set_active_id(selected)):
            self.source.set_active(0)

    def show_source(self):
        source = self.source.get_active_id()
        if source:
            ref = re.sub(r"(#\d+)[./]\d+$", r"\1", source)
            self.command("/show " + ref)

    def correct_source(self):
        source = self.source.get_active_id()
        if source:
            self.editor.get_buffer().set_text("/amend " + source + " ")
            self.navigate("Lesson")
            self.editor.grab_focus()

    def revise_work(self):
        if self.assignment and self.assignment["submissions"]:
            self.editor.get_buffer().set_text(self.assignment["submissions"][-1]["work"])
            self.editor.grab_focus()

    def pending_label(self):
        pending = self.assignment and self.outbox.exists()
        self.shared.set_text("Submission pending acknowledgment" if pending else
                             ("Submission acknowledged" if self.assignment and self.assignment["submissions"] else ""))
        self.submit.set_label("Retry submission" if pending else "Preview submission")

    def stop(self):
        self.epoch += 1
        if self.proc and self.proc.poll() is None:
            if self.ready:
                self.proc.stdin.close()
                try:
                    self.proc.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    pass
            if self.proc.poll() is None:
                if os.name == "nt":
                    subprocess.run(["taskkill", "/PID", str(self.proc.pid), "/T", "/F"],
                                   capture_output=True, creationflags=subprocess.CREATE_NO_WINDOW)
                else:
                    os.killpg(self.proc.pid, signal.SIGTERM)
                self.proc.wait()
        self.ready = False
        self.proc = None

    def choose(self, picker):
        if getattr(self, "updating", False):
            return
        self.stop()
        self.show_page("Lesson")
        self.key = picker.get_active_id()
        save(self.last_lesson, self.key)
        self.revision = None
        self.form = None
        self.sent_at = None
        self.activity.restore(None, {"fields": {}})
        self.plan_view.get_buffer().set_text("")
        self.plan_panel.set_expanded(False)
        self.corrections.set_visible(False)
        self.revise.set_visible(False)
        self.assignment = None
        self.shared.set_text("")
        self.lesson_title.set_label("Your lesson")
        self.lesson_title.set_tooltip_text(None)
        self.context.set_text(PRESETS[self.test_learner][1] if self.test_learner else "Personal lesson")
        self.folder = self.personal
        if not self.test_learner and self.key.isdigit():
            self.folder = self.cache / self.key
            self.outbox = self.folder / "pending.json"
            self.load_assignment()
            try:
                api("study", id=int(self.key))
                a = next(
                    a for a in api("state")["assignments"] if a["id"] == int(self.key)
                )
                save(self.folder / "assignment.json", a)
                self.load_assignment()
            except (OSError, ValueError, StopIteration):
                pass
            binding = self.folder / "lesson.assignment.json"
            pinned = {k: self.assignment[k] for k in ("id", "version", "activity")}
            if binding.exists() and json.loads(binding.read_text()) != pinned:
                raise ValueError("Assignment differs from its saved lesson binding")
            if not binding.exists():
                save(binding, pinned)
        self.path = self.folder / ("lesson.jsonl" if self.assignment else self.key)
        self.submit.set_visible(bool(self.assignment))
        self.editor.get_buffer().set_text("")
        self.history.get_buffer().set_text("")
        self.set_quick_replies([])
        self.exercise.set_question("")
        self.reason.set_text("")
        self.private_courses.set_working_plan(None)
        self.private_courses.bind(self.personal)
        self.inspector.bind(self.path)
        self.pending_turn = False
        self.offset = 0
        self.locked = False
        self.start()

    def start(self):
        self.epoch += 1
        self.ready = False
        self.started = time.monotonic()
        self.note = "Opening lesson"
        env = dict(os.environ, ROBOTUTOR_DESKTOP="1")
        review_args = ["--astra"] if env.get("ROBOTUTOR_ASTRA_REVIEW") == "1" else []
        if self.test_learner:
            env.update(ROBOTUTOR_WORKSPACE=str(self.personal), ROBOTUTOR_ASSIGNMENTS=str(self.cache),
                       ROBOTUTOR_PORT="1")
        self.proc = subprocess.Popen(
            [
                os.environ["ROBOTUTOR_BIN"],
                *review_args,
                "--workspace",
                self.folder,
                "--session",
                self.path.name,
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            env=env,
            **background_options(),
        )

        def read(proc, epoch):
            for line in proc.stdout:
                self.output.put((epoch, line.decode(errors="replace").rstrip("\n")))

        threading.Thread(target=read, args=(self.proc, self.epoch), daemon=True).start()

    def write_frame(self, frame):
        self.proc.stdin.write((json.dumps(frame) + "\n").encode())
        self.proc.stdin.flush()

    def open_assignment(self):
        from sustained_window import AssignmentWindow
        import sqlite3
        try:
            plan = self.private_courses.working_plan or {}
            topic = self.assignment["activity"]["title"] if self.assignment else plan.get("subject", "")
            AssignmentWindow(self.win, self.folder, topic).present()
        except (OSError, sqlite3.Error) as exc:
            self.note = f"Could not open the saved assignment: {exc}"
            self.chat(self.note)

    def save_draft(self):
        if self.restoring or not self.ready or self.pending_turn or self.locked or self.revision is None:
            return
        try:
            self.write_frame({"turn": self.revision, "draft": {
                "text": words(self.editor), "fields": self.activity.values()}})
        except OSError:
            self.ready = False
            self.note = "Tutor disconnected while saving the draft"

    def command(self, text):
        if not self.ready or not text.strip():
            return False
        self.ready = False
        self.note = "Waiting for Luna"
        self.plan_requested = text.split(" ", 1)[0] in {"/plan", "/skills", "/show", "/trace", "/recall", "/work", "/activities", "/hint", "/tap", "/run", "/draft", "/help"}
        if not self.plan_requested:
            self.sent_at = self.started = time.monotonic()
            self.sent_revision = self.revision or 0
            self.first_feedback = None
            self.sent_kind = ("return" if text == "/retry" else "correction" if text.startswith("/amend ")
                              else "practice" if self.form else "ordinary")
        self.started = time.monotonic()
        try:
            if text == "/send":
                self.write_frame({"turn": self.revision, "submit": {
                    "text": words(self.editor), "fields": self.activity.values()}})
            else:
                self.write_frame(text)
            return True
        except OSError:
            self.note = "Tutor stopped"
            return False

    def preedit_changed(self, _view, text):
        self.composing = bool(text)
        self.update_composer()

    def answer_key(self, _controller, key, _code, modifiers):
        if (key in (Gdk.KEY_Return, Gdk.KEY_KP_Enter)
                and not self.composing
                and not modifiers & (Gdk.ModifierType.SHIFT_MASK | Gdk.ModifierType.ALT_MASK)):
            if self.send_button.get_sensitive():
                self.send()
            return True
        return False

    def answer_changed(self, *_):
        self.update_composer()
        self.save_draft()

    def update_composer(self):
        self.answer_hint.set_visible(not self.composing and self.editor.get_buffer().get_char_count() == 0)
        writable = self.ready and not self.locked and not self.pending_turn
        self.quick_replies.set_sensitive(bool(writable and not words(self.editor) and not self.composing))
        self.send_button.set_sensitive(bool(writable and (self.form or words(self.editor).strip())))

    def set_quick_replies(self, choices):
        while child := self.quick_replies.get_first_child():
            self.quick_replies.remove(child)
        for choice in choices:
            def pick(value=choice):
                if self.ready and not self.pending_turn and not words(self.editor):
                    self.editor.get_buffer().set_text(value)
                    self.send()
                    self.update_composer()
            self.button(self.quick_replies, choice, pick)
        self.quick_replies.set_visible(bool(choices))
        self.answer_hint.set_text("Or type your own reply…" if choices else "Reply to Luna…")

    def send(self):
        if not self.ready or self.locked or self.pending_turn:
            return
        text = words(self.editor).strip()
        if text.startswith("/"):
            self.editor.get_buffer().set_text("")
            self.command(text)
        elif self.form or text:
            self.command("/send")

    def record(self, row):
        stage = row.get("event", "")
        if stage == "workspace_draft":
            return
        if stage in ("practice_answer", "learner", "failed", "resumed_interrupted", "delivered"):
            self.pending_turn = stage != "delivered"
        lesson = row.get("lesson") or row.get("continuation") or {}
        if "exercise" in lesson:
            discovery = lesson.get("discovery") or {}
            active = discovery.get("phase") in {"learner", "reflection", "subject", "review"}
            title = (self.assignment["activity"]["title"] if self.assignment else
                     "Getting to know you" if active else lesson.get("objective") or "Your lesson")
            self.lesson_title.set_label(title)
            self.lesson_title.set_tooltip_text(title)
            question = discovery["question"] if active else lesson.get("current_step") or lesson["exercise"]
            recall = lesson.get("name_recall") == "asking" or any(
                item["state"] == "asking" for item in lesson.get("recall_items", []))
            if recall and not active:
                # Historical deliveries carry the actual recall prompt at the end of the reply.
                text = row.get("text")
                if stage == "recovered" and row["exchanges"]:
                    text = row["exchanges"][-1][1]
                if text:
                    self.exercise.set_question(text.rstrip().rsplit("\n\n", 1)[-1])
            else:
                self.exercise.set_question(question)
            reason = discovery.get("reason") if active or (stage == "delivered" and not row.get("judgment")) else None
            evidence = lesson.get("practice_evidence")
            if evidence:
                reason = evidence["feedback"] + " (" + evidence["source"] + ")"
            self.reason.set_text(reason or "")
        if stage == "delivered":
            self.chat(row["text"], "Luna", self.exercise.get_text(), lesson.get("objective", ""))
        elif stage in ("learner", "practice_answer"):
            self.chat(row["text"], "You")
        if stage == "supervisor_plan" and row.get("replace_prompt"):
            self.exercise.set_question(row["task"])
        if stage == "recovered":
            for learner, tutor in row["exchanges"]:
                if learner:
                    self.chat(learner, "You")
                self.chat(tutor, "Luna")

    def chat(self, text, speaker=None, prompt="", objective=""):
        buf = self.history.get_buffer()
        adj = self.history_scroll.get_vadjustment()
        following = adj.get_value() + adj.get_page_size() >= adj.get_upper() - 6
        follow = following or self.sent_at is not None
        if follow and speaker == "Luna":
            start = buf.get_mark("reply-start")
            if start is None:
                start = buf.create_mark("reply-start", buf.get_end_iter(), True)
            else:
                buf.move_mark(start, buf.get_end_iter())
        if speaker:
            self.history.write(speaker + "\n", "speaker")
        if speaker == "Luna":
            self.history.format_tutor(text, prompt, objective)
        else:
            self.history.write(text + "\n")
        self.history.write("\n")
        if follow:
            if speaker == "Luna":
                # Show the start of a long reply after GTK has laid out the new text.
                self.history.scroll_to_mark(start, 0, True, 0, 0)
            else:
                self.history.scroll_to_iter(buf.get_end_iter(), 0, False, 0, 1)

    def fit_window(self):
        width = self.win.get_width()
        narrow = 0 < width < 880
        if narrow == self.narrow:
            return
        self.narrow = narrow
        self.notes_button.set_visible(narrow)
        self.compact_reason.set_visible(narrow and bool(self.reason.get_text()))
        focus = self.win.get_focus()
        if focus and (focus.is_ancestor(self.inspector) or focus.is_ancestor(self.rail)):
            self.explorer_button.grab_focus()
        if narrow:
            self.sidebar.set_start_child(None)
            self.pages.add_named(self.rail, "Notes")
        else:
            if self.pages.get_visible_child_name() == "Notes":
                self.show_page("Lesson")
            self.pages.remove(self.rail)
            self.sidebar.set_start_child(self.rail)
        self.layout_lesson_panes()

    def layout_lesson_panes(self):
        opened = self.explorer_button.get_active()
        self.answer_scroll.set_min_content_height(32 if self.narrow and opened else 62)
        self.inspector.set_compact(self.narrow)
        self.inspector.set_visible(opened)
        self.sidebar.set_visible(opened or not self.narrow)
        self.split.set_wide_handle(opened)
        self.split.set_orientation(Gtk.Orientation.VERTICAL if self.narrow else Gtk.Orientation.HORIZONTAL)
        if self.narrow:
            self.split.set_position(round(self.split.get_height() * 0.60))
        else:
            self.sidebar.set_wide_handle(opened)
            self.sidebar.set_position(round(self.split.get_height() * 0.34))
            self.split.set_position(round(self.split.get_width() * (0.58 if opened else 0.68)))
        self.split.get_start_child().set_margin_end(0 if self.narrow else 12)
        self.split.get_start_child().set_margin_bottom(6 if self.narrow and opened else 0)

    def fit_question(self):
        panel, scroll = self.question_panel, self.question_scroll
        if not scroll.get_mapped() or not panel.get_expanded():
            return
        column = panel.get_parent()
        width = column.get_width()
        vertical = Gtk.Orientation.VERTICAL
        # GTK measures the other controls at this width, including a wrapped
        # reply or an open activity. Reserve those heights and the history
        # minimum before letting the formatted question claim any space.
        available = column.get_height()
        visible = [child for child in column if child.get_visible()]
        available -= column.get_spacing() * (len(visible) - 1)
        for child in visible:
            if child == panel:
                available -= panel.get_allocated_height() - scroll.get_allocated_height()
            elif child == self.history_scroll:
                available -= child.measure(vertical, width).minimum
            else:
                available -= child.measure(vertical, width).natural
        wrapped = self.exercise.measure(vertical, self.exercise.get_width()).natural
        height = max(1, min(wrapped, available))
        if height != scroll.get_max_content_height():
            scroll.set_max_content_height(height)

    def tick(self):
        self.fit_window()
        self.fit_question()
        if not self.proc:
            return False
        notices = []
        ready_trace = None
        ready_question = None
        while not self.output.empty():
            epoch, line = self.output.get()
            if epoch != self.epoch:
                continue
            if line.startswith("\x1eREADY "):
                snapshot = json.loads(line.split(" ", 1)[1])
                ready_question = snapshot["exercise"]
                self.set_quick_replies(snapshot.get("choices", []))
                if not self.assignment:
                    self.private_courses.set_working_plan(snapshot.get("working_plan"))
                seed = snapshot.get("course_seed")
                if seed and not self.assignment and not self.path.name.startswith("course-activity-"):
                    self.private_courses.propose(seed)
                reason = snapshot.get("discovery_reason")
                if reason:
                    self.reason.set_text(reason)
                workspace = snapshot.get("workspace", {})
                if self.revision is not None and workspace.get("turn") != self.revision:
                    self.plan_panel.set_expanded(False)
                    self.plan_view.get_buffer().set_text("")
                if not self.plan_panel.get_expanded():
                    self.set_sources(workspace.get("sources", []))
                self.form = workspace.get("form")
                self.activity_scroll.set_visible(bool(self.form))
                if self.form:
                    self.answer_hint.set_text("Add a note about your work…")
                self.revision = workspace.get("turn")
                if workspace.get("trace"):
                    ready_trace = workspace["trace"]
                self.restoring = True
                draft = workspace.get("draft", {"text": "", "fields": {}})
                if words(self.editor) != draft["text"]:
                    self.editor.get_buffer().set_text(draft["text"])
                self.activity.restore(self.form, draft)
                self.restoring = False
                self.pending_turn = snapshot["pending"]
                self.ready = True
                self.locked = False
                self.note = (
                    "Message pending · Retry to resume"
                    if getattr(self, "pending_turn", False)
                    else "Ready"
                )
                if getattr(self, "plan_requested", False):
                    self.plan_requested = False
            elif line.startswith("\x1eSUBMISSION "):
                if not json.loads(line.split(" ", 1)[1])["pending"]:
                    self.refresh()
                self.pending_label()
            elif line.startswith("\x1ePLAN "):
                plan = json.loads(line.split(" ", 1)[1])
                self.plan_view.get_buffer().set_text(plan["text"])
                self.plan_panel.set_expanded(True)
                self.set_sources(plan.get("sources", []))
                self.chat(plan["text"])
            elif line.startswith("\x1eFEEDBACK "):
                self.chat(json.loads(line.split(" ", 1)[1]), "Luna")
                if self.sent_at and self.first_feedback is None:
                    self.first_feedback = int((time.monotonic() - self.sent_at) * 1_000_000)
            elif line.startswith("\x1eNOTICE "):
                notices.append(json.loads(line.split(" ", 1)[1]))
            elif "already open or cannot be locked" in line:
                self.locked = True
                self.note = "Read only · another tutor owns this journal"
            elif line:
                self.note = line[:300]
                if getattr(self, "plan_requested", False):
                    self.chat(line)
                if line.startswith("Luna ·"):
                    self.record({"event": "status", "message": line})
        try:
            if os.name == "nt":
                if self.path.stat().st_size > self.offset:
                    run = subprocess.run([os.environ["ROBOTUTOR_BIN"], "--lesson-events", self.path,
                                          str(self.offset), "128"], capture_output=True, timeout=5,
                                         creationflags=subprocess.CREATE_NO_WINDOW)
                    if run.returncode:
                        raise ValueError(run.stderr.decode("utf-8", errors="replace").strip())
                    for line in run.stdout.splitlines():
                        event = json.loads(line)
                        self.record(event["record"])
                        self.offset = event["next_offset"]
            else:
                with open(
                    self.path, "rb", opener=lambda p, f: os.open(p, f | os.O_NOFOLLOW)
                ) as journal:
                    info = os.fstat(journal.fileno())
                    if (
                        not stat.S_ISREG(info.st_mode)
                        or info.st_mode & 0o077
                        or info.st_nlink != 1
                    ):
                        raise ValueError("Journal must be private")
                    journal.seek(self.offset)
                    for line in journal:
                        if not line.endswith(b"\n"):
                            break
                        self.record(json.loads(line))
                        self.offset += len(line)
        except FileNotFoundError:
            pass
        except (ValueError, OSError, subprocess.TimeoutExpired) as err:
            self.note = str(err)
        if ready_question is not None:
            self.exercise.set_question(ready_question)
        if ready_trace:
            self.reason.set_text(ready_trace)
        for notice in notices:
            self.chat(notice, "Luna")
        if self.proc.poll() is not None:
            self.ready = False
            if self.locked and time.monotonic() - self.started > 3:
                self.start()
        elapsed = (
            ""
            if self.ready or self.locked
            else f" · {int(time.monotonic() - self.started)}s"
        )
        allowed = self.assignment and (
            self.outbox.exists()
            or self.assignment["status"] not in {"Accepted", "Awaiting review"}
        )
        self.submit.set_sensitive(bool(self.ready and not self.locked and allowed))
        self.status.set_text(
            (
                "Message pending · Retry to resume"
                if self.ready and self.pending_turn
                else self.note
            )
            + elapsed
        )
        if self.sent_at and self.ready:
            if not self.pending_turn and self.revision is not None and self.revision > self.sent_revision:
                started, turn, kind = self.sent_at, self.revision, self.sent_kind
                epoch = self.epoch
                feedback = self.first_feedback
                def rendered(*_):
                    if self.epoch == epoch and self.proc and self.proc.poll() is None:
                        us = int((time.monotonic() - started) * 1_000_000)
                        try:
                            self.write_frame({"rendered": {"turn": turn, "kind": kind,
                                "feedback_us": feedback if feedback is not None else us,
                                "exercise_us": us if self.exercise.get_text() else None}})
                        except OSError:
                            pass
                    return False
                self.win.add_tick_callback(rendered)
            self.sent_at = None
        self.update_composer()
        self.activity.set_sensitive(self.ready and not self.pending_turn)
        self.editor.set_editable(self.ready and not self.locked and not self.pending_turn)
        self.actions.set_sensitive(self.ready)
        self.controls[0].set_visible(bool(self.pending_turn))
        for button in self.controls:
            button.set_sensitive(self.ready)
        return True

    def import_document(self):
        old = self.pages.get_child_by_name("Import")
        if old:
            self.pages.remove(old)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        box.add_css_class("workspace-body")
        box.append(self.heading("Open a text document", "page-title"))
        chooser = Gtk.FileChooserWidget(action=Gtk.FileChooserAction.OPEN, vexpand=True)
        box.append(chooser)
        actions = Gtk.Box(spacing=8)
        box.append(actions)
        def import_selected():
            selected = chooser.get_file()
            if selected and selected.get_path() and self.command("/import " + selected.get_path()):
                self.show_page("Lesson")
        self.button(actions, "Open document", import_selected).add_css_class("suggested-action")
        self.button(actions, "Cancel", lambda: self.show_page("Lesson"))
        self.pages.add_named(box, "Import")
        self.pages.set_visible_child_name("Import")

    def preview(self):
        if self.outbox.exists():
            return self.transmit()
        old = self.pages.get_child_by_name("Submission")
        if old:
            self.pages.remove(old)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        box.add_css_class("workspace-body")
        self.pages.add_named(box, "Submission")
        box.append(self.heading("Preview submission", "page-title"))
        box.append(Gtk.Label(label="Only the text below will be shared with your teacher.", xalign=0, wrap=True))
        chosen = view(True)
        entries = self.activity.values() if self.form else {}
        preview = [f"{f['label']}: {entries[f['id']]}" for f in (self.form or {}).get("fields", [])
                   if entries.get(f["id"])]
        if words(self.editor):
            preview.append(words(self.editor))
        source_text = "\n".join(preview)
        journal = self.path
        previous = self.preview_drafts.get(journal)
        chosen.get_buffer().set_text(previous[1] if previous and previous[0] == source_text else source_text)
        def remember_preview(*_):
            self.preview_drafts[journal] = (source_text, words(chosen))
        chosen.get_buffer().connect("changed", remember_preview)

        box.append(pane(chosen))

        def confirm():
            work = words(chosen)
            if work.strip():
                if self.command("/confirm-preview " + work):
                    dismiss()

        actions = Gtk.Box(spacing=8)
        box.append(actions)
        def dismiss():
            self.preview_drafts.pop(journal, None)
            self.show_page("Lesson")
        self.button(actions, "Submit this text", confirm).add_css_class("suggested-action")
        self.button(actions, "Cancel", dismiss)
        self.pages.set_visible_child_name("Submission")
        chosen.grab_focus()

    def transmit(self):
        self.command("/submit")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--test", choices=PRESETS, help="Open an isolated fictional learner")
    parser.add_argument("--reset", action="store_true", help="Start this test learner again")
    parser.add_argument("assignment", nargs="?")
    args = parser.parse_args()
    if (args.reset and not args.test) or (args.test and args.assignment):
        parser.error("--reset requires --test, and assignments belong in My workspace")
    forwarded = ["--test", args.test] if args.test else [args.assignment] if args.assignment else []
    if args.reset:
        forwarded.append("--reset")
    sys.exit(Pilot(register_desktop=True).run([sys.argv[0], *forwarded]))
