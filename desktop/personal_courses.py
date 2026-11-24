"""Private course outlines and activity journals owned by one learner workspace."""
import json
from pathlib import Path
import threading

import gi
gi.require_version("Gtk", "4.0")
from gi.repository import GLib, Gtk
from course_agent import atomic, digest, save_personal


def open_activity(folder, key, course, activity):
    name = "course-activity-" + digest([key, activity.get("key", activity["title"])]) + ".jsonl"
    exercise = activity["instructions"] + "\n\n" + activity["criterion"]
    lesson = dict(topic=course["title"], declared_goal=course["destination"], selected=None,
                  stage=0, objective=activity["objective"], next_objective="", exercise=exercise,
                  closed=[], challenge=False, washington_mentioned=False)
    rows = [dict(event="open", version=1, selected=None),
            dict(event="delivered", text=exercise, lesson=lesson, sources=[])]
    atomic(folder / name, "".join(json.dumps(r) + "\n" for r in rows), create=True)
    return name


class PersonalCourses(Gtk.ScrolledWindow):
    def __init__(self, open_session):
        super().__init__(vexpand=True)
        self.open_session = open_session
        self.folder = None
        self.busy = set()
        self.working_plan = None
        self.content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8,
                               margin_top=16, margin_bottom=16, margin_start=20, margin_end=20)
        self.set_child(self.content)

    def set_working_plan(self, plan):
        if self.working_plan != plan:
            self.working_plan = plan
            self.render()

    def bind(self, folder):
        self.folder = folder
        self.render()

    def propose(self, seed, request=None):
        directory = self.folder / ".courses"
        directory.mkdir(exist_ok=True, mode=0o700)
        path = directory / (digest(seed["goal"]) + ".json")
        if path in self.busy or (request is None and path.exists()):
            return
        self.busy.add(path)
        self.render()

        def generate():
            try:
                save_personal(path, seed, request)
            finally:
                GLib.idle_add(self.finished, path)
        threading.Thread(target=generate, daemon=True).start()

    def finished(self, path):
        self.busy.discard(path)
        self.render()
        return False

    def render(self):
        while child := self.content.get_first_child():
            self.content.remove(child)
        def label(text, css=None):
            widget = Gtk.Label(label=text, xalign=0, wrap=True)
            if css:
                widget.add_css_class(css)
            self.content.append(widget)
        def button(text, fn):
            widget = Gtk.Button(label=text, halign=Gtk.Align.START)
            widget.connect("clicked", lambda *_: fn())
            self.content.append(widget)
        if not self.folder:
            return
        folder = self.folder
        directory = folder / ".courses"
        paths = sorted(directory.glob("*.json"))
        active = any(p.parent == directory for p in self.busy)
        plan = self.working_plan
        if plan:
            label(plan["subject"] or "Choosing a subject", "page-title")
            start = plan.get("suggested_start")
            if start:
                label(("Agreed starting point" if plan["phase"] == "instruction" else "Suggested starting point")
                      + " · " + start["objective"]
                      + (f' ({plan["agreement_source"]})' if plan.get("agreement_source") else ""))
                label(start["why"])
            for agreement in plan.get("agreements", []):
                if start and agreement["source"] == plan.get("agreement_source"):
                    continue
                status = "Earlier starting point, since corrected" if agreement["evidence_since_corrected"] else "You agreed to try"
                label(f'{status} · {agreement["tutor_proposal"]["objective"]} ({agreement["source"]})')
            if not start and plan["phase"] != "instruction":
                label(plan["question"])
            route = plan.get("route")
            if route and route["milestones"]:
                label(route["destination"])
                panel = Gtk.Expander(label="Course route")
                milestones = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
                for i, milestone in enumerate(route["milestones"]):
                    text = milestone["skill"]
                    if i == route["focus"]:
                        text += " · current focus"
                    milestones.append(Gtk.Label(label=text, xalign=0, wrap=True))
                panel.set_child(milestones)
                self.content.append(panel)
            step = plan.get("teaching_step") or plan.get("practice")
            if step:
                label(plan.get("current_objective") or "Current step", "section-title")
                label((step.get("reason") or step["feedback"]) + f' ({step["source"]})')
            for observation in plan["observations"]:
                status = {"reported": "You reported", "independent": "You demonstrated",
                          "cued": "With a cue", "correction": "You corrected"}.get(observation["kind"], observation["kind"].capitalize())
                if observation["superseded"]:
                    status = "Earlier reading, since corrected"
                label(f'{status} · “{observation["learner_quote"]}” ({observation["source"]})')
        if active:
            label("Preparing your course…")
        elif not paths and not plan:
            label("Your private course appears here when you and Luna agree on a learning goal.")
        for path in paths:
            state = json.loads(path.read_text())
            course = state["outline"]
            label(course["title"] if course else state["seed"]["goal"], "page-title")
            if course:
                label(course["destination"])
                for unit in course["units"]:
                    label(unit["title"], "section-title")
                    for activity in unit["activities"]:
                        label(activity["objective"])
                        button(activity["title"], lambda a=activity, c=course, k=path.stem:
                               self.open_session(open_activity(folder, k, c, a)))
            if state.get("direction") and state.get("changes"):
                label(state["changes"])
            if state.get("problem"):
                label(state["problem"])
            if path not in self.busy:
                entry = Gtk.Entry(placeholder_text="How would you like to change your course?")
                self.content.append(entry)
                seed = state["seed"]
                if plan and (plan["subject"] == seed["goal"] or
                             plan["subject"] == seed.get("working_plan", {}).get("subject")):
                    seed = dict(seed, working_plan=plan)
                button("Revise course" if course else "Retry proposal",
                       lambda e=entry, s=seed: self.propose(s, e.get_text().strip()))
