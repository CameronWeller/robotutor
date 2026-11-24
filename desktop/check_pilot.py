#!/usr/bin/python3
"""Run the browser and native acceptance flow in an isolated private workspace."""

import json
import os
import subprocess
import sys
import tempfile
import time
from urllib.request import Request, urlopen
from pathlib import Path

root = Path(__file__).resolve().parents[1]
tmp = tempfile.TemporaryDirectory(prefix="robotutor-acceptance-")
work = Path(tmp.name)
port = os.environ.get("PILOT_TEST_PORT", "47322")
os.environ.update(
    ROBOTUTOR_PORT=port,
    ROBOTUTOR_ASSIGNMENTS=str(work / "assigned"),
    ROBOTUTOR_WORKSPACE=str(work / "personal"),
    PILOT_URL="http://127.0.0.1:" + port,
)
os.environ["ROBOTUTOR_ASTRA_REVIEW"] = "0"
from app import GLib, Gtk, Pilot, api, words


def service():
    proc = subprocess.Popen(
        [
            Path(os.environ["ROBOTUTOR_BIN"]).with_name("robotutor-lms"),
            work / "courses.db",
            root / "web",
        ]
    )
    for _ in range(80):
        if proc.poll() is not None:
            raise RuntimeError("Test port unavailable")
        try:
            api("state")
            return proc
        except OSError:
            time.sleep(0.05)
    raise RuntimeError("Service failed to start")


def wait(condition, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        while GLib.MainContext.default().pending():
            GLib.MainContext.default().iteration(False)
        if condition():
            return
        time.sleep(0.03)
    raise RuntimeError(
        "Timed out waiting for " + str(condition.__code__.co_firstlineno)
    )


def web(*args):
    subprocess.run(["node", root / "web/tests/workflow.cjs", *args], check=True)


def buttons(widget):
    if isinstance(widget, Gtk.Button):
        yield widget
    child = widget.get_first_child()
    while child:
        yield from buttons(child)
        child = child.get_next_sibling()


def evaluate(web_view, code):
    result = []
    def finished(web, task, _):
        result.append(web.evaluate_javascript_finish(task).to_string())
    web_view.evaluate_javascript(code, -1, None, None, None, finished, None)
    wait(lambda: result)
    return result[0]


def events():
    with app.path.open() as journal:
        return [json.loads(line) for line in journal if line.endswith("\n")]


def submit(text):
    app.editor.get_buffer().set_text(text)
    app.preview()
    dialog = app.pages.get_visible_child()
    assert app.pages.get_visible_child_name() == "Submission"
    assert len(app.get_windows()) == 1
    next(b for b in buttons(dialog) if b.get_label() == "Submit this text").emit(
        "clicked"
    )
    wait(lambda: app.ready)


srv = service()
app = None
tui = None
try:
    web()
    app = Pilot()
    app.register(None)
    app.activate()
    wait(lambda: app.ready)
    window = app.win
    app.show_page("Courses")
    course_view = app.courses.web
    wait(lambda: course_view and course_view.get_estimated_load_progress() == 1)
    evaluate(course_view, "document.querySelector('#profile').value='learner'; document.querySelector('#profile').dispatchEvent(new Event('change'));")
    wait(lambda: evaluate(course_view, "!!document.querySelector('a.open')") == "true")
    evaluate(course_view, "document.querySelector('a.open').click(); 'opened'")
    wait(lambda: app.ready and app.key == "1" and app.pages.get_visible_child_name() == "Lesson")
    assert app.win is window and len(app.get_windows()) == 1
    assert app.assignment["version"] == 1 and app.assignment["status"] == "In progress"
    assert "Share a four-beat count" in app.context.get_text()
    private = "Help me count four beats.\nI am working on the pulse."
    # Force a recoverable credentials failure before the real retry.
    original_path = os.environ["PATH"]
    os.environ["PATH"] = str(work)
    original_key = os.environ.pop("OPENROUTER_API_KEY", None)
    app.stop()
    app.start()
    wait(lambda: app.ready)
    app.editor.get_buffer().set_text(private)
    app.send()
    wait(lambda: any(e["event"] == "failed" for e in events()))
    app.stop()
    os.environ["PATH"] = original_path
    if original_key:
        os.environ["OPENROUTER_API_KEY"] = original_key
    args = [
        os.environ["ROBOTUTOR_BIN"],
        "--workspace",
        app.folder,
        "--session",
        app.path.name,
    ]
    tui = subprocess.Popen(
        args,
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    time.sleep(0.5)
    app.start()
    wait(lambda: app.locked)
    assert private in words(app.history)
    tui.stdin.close()
    tui.wait(timeout=5)
    tui = None
    wait(lambda: app.ready and not app.locked)
    assert app.pending_turn
    app.command("/retry")
    wait(lambda: app.ready, 180)
    delivered = [e for e in events() if e["event"] == "delivered"][1:]
    if os.getenv("PILOT_LIVE") == "1":
        failure = next(
            (e["message"] for e in reversed(events()) if e["event"] == "failed"),
            "Missing discovery reply",
        )
        assert delivered and delivered[-1].get("lesson", {}).get("discovery"), failure
        assert private in words(app.history)
        assert app.reason.get_text()
        app.command("/plan")
        wait(lambda: app.ready)
        assert "Getting to know you" in words(app.history)
    app.stop()
    app.key = None
    app.refresh()
    wait(lambda: app.ready)
    assert private in words(app.history)
    srv.terminate()
    srv.wait()
    submit("  ONE two three four\n")
    assert app.outbox.exists() and "pending" in app.shared.get_text()
    srv = service()
    # Simulate a successful POST whose response the desktop never received.
    ack = api("submit", **json.loads(app.outbox.read_text()))
    app.transmit()
    wait(lambda: app.ready)
    assert not app.outbox.exists()
    a = api("state")["assignments"][0]
    assert a["status"] == "Awaiting review" and len(a["submissions"]) == 1
    assert a["submissions"][0]["work"] == "  ONE two three four\n"
    assert private not in json.dumps(a)
    teacher = Request(
        os.environ["PILOT_URL"] + "/api",
        b'{"action":"state"}',
        {"Content-Type": "application/json", "X-Profile": "teacher"},
    )
    with urlopen(teacher) as response:
        shared = json.load(response)
    assert shared["assignments"][0]["submissions"] == a["submissions"]
    assert private not in json.dumps(shared)
    if delivered:
        discovery = delivered[-1]["lesson"]["discovery"]
        assert discovery["reason"] not in json.dumps(shared)
        assert all(f["quote"] not in json.dumps(shared) for f in discovery["profile"]["facts"].values())
    app.navigate("Courses")
    web("--review")
    app.navigate("Lesson")
    assert "Explain the second beat." in app.context.get_text()
    app.stop()
    recovered = subprocess.run(args, input="/assignment\n/revision\n/draft\n/quit\n",
                               text=True, capture_output=True, check=True, timeout=15)
    assert "Explain the second beat." in recovered.stdout and "ONE two three four" in recovered.stdout
    app.start()
    wait(lambda: app.ready)
    assert words(app.editor) == "  ONE two three four\n"
    submit("ONE two three four. Beat two is unaccented.")
    web("--accept")
    app.refresh()
    assert app.assignment["status"] == "Accepted"
    print(
        f"Browser and GTK workflow passed. Delivered Luna turns: {len(delivered)}. Acknowledged revisions: 2."
    )
finally:
    if tui and tui.poll() is None:
        tui.terminate()
        tui.wait()
    if app:
        app.stop()
        if hasattr(app, "win"):
            app.win.destroy()
    if srv.poll() is None:
        srv.terminate()
        srv.wait()
