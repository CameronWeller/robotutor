"""Keep the local course manager and its assignment links in the lesson window."""
from pathlib import Path
import re
import os
import subprocess
import sys
import threading

import gi
from gi.repository import GLib, Gtk


class Courses(Gtk.Box):
    def __init__(self, url, open_assignment):
        super().__init__(orientation=Gtk.Orientation.VERTICAL)
        self.url = url
        self.native_courses = sys.platform in {"win32", "darwin"} and bool(os.getenv("ROBOTUTOR_COURSES_BIN"))
        self.open_assignment = open_assignment
        self.web = None
        self.native_window = None
        self.error = Gtk.Label(wrap=True, margin_top=28)
        self.append(self.error)
        self.retry = Gtk.Button(label="Reload courses", halign=Gtk.Align.CENTER, margin_top=12)
        self.retry.connect("clicked", lambda *_: self.open_native() if self.native_courses else self.load())
        self.append(self.retry)

    def load(self):
        if self.native_courses:
            self.retry.set_label("Open course manager")
            self.error.set_label("Manage class courses and open an assignment in Robotutor.")
            return
        if not self.web:
            # A user-local installation of the system introspection bindings is supported.
            local = Path(GLib.get_user_data_dir()) / "girepository-1.0"
            if local.is_dir():
                gi.Repository.get_default().prepend_search_path(str(local))
            try:
                gi.require_version("WebKit", "6.0")
                from gi.repository import WebKit
            except (ValueError, ImportError):
                self.error.set_label("The course view needs the WebKitGTK 6.0 Python bindings.")
                return
            self.web = WebKit.WebView(vexpand=True)
            self.append(self.web)
            def navigate(_, decision, kind):
                if kind in (WebKit.PolicyDecisionType.NAVIGATION_ACTION, WebKit.PolicyDecisionType.NEW_WINDOW_ACTION):
                    uri = decision.get_navigation_action().get_request().get_uri()
                    match = re.fullmatch(r"robotutor://assignment/(\d+)", uri)
                    if match:
                        decision.ignore()
                        GLib.idle_add(lambda: self.open_assignment(match[1]) or False)
                        return True
                    if not uri.startswith(self.url + "/"):
                        decision.ignore()
                        return True
                return False
            self.web.connect("decide-policy", navigate)
            self.web.connect("load-failed", self.failed)
            css = WebKit.UserStyleSheet.new("header{padding:8px 4vw;justify-content:flex-end} header strong{display:none}", WebKit.UserContentInjectedFrames.TOP_FRAME, WebKit.UserStyleLevel.USER, None, None)
            self.web.get_user_content_manager().add_style_sheet(css)
        self.error.set_visible(False)
        self.retry.set_visible(False)
        self.web.set_visible(True)
        self.web.load_uri(self.url + "/")

    def open_native(self):
        if self.native_window is not None:
            return
        flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
        try:
            proc = subprocess.Popen([os.environ["ROBOTUTOR_COURSES_BIN"]],
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, creationflags=flags)
        except OSError as err:
            self.error.set_text(str(err))
            return
        self.native_window = proc
        self.retry.set_sensitive(False)

        def closed():
            output, error = proc.communicate()
            def finish():
                self.native_window = None
                self.retry.set_sensitive(True)
                assignment = output.decode("ascii", errors="replace").strip()
                if proc.returncode:
                    self.error.set_text(error.decode("utf-8", errors="replace").strip()[:300] or "Could not open courses.")
                elif re.fullmatch(r"[0-9]+", assignment):
                    self.open_assignment(assignment)
                return False
            GLib.idle_add(finish)
        threading.Thread(target=closed, daemon=True).start()

    def failed(self, _, event, uri, error):
        if uri.startswith(self.url + "/"):
            self.web.set_visible(False)
            self.error.set_label("The local course service is unavailable. Your lesson is still open.")
            self.error.set_visible(True)
            self.retry.set_visible(True)
            return True
        return False
