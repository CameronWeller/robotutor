#!/usr/bin/python3
"""Run cross-client draft, mixed-evidence and correction journeys in a disposable GTK lesson."""
import json
import os
import subprocess
import tempfile
import time
from pathlib import Path

with tempfile.TemporaryDirectory(prefix="robotutor-workspace-") as tmp:
    root = Path(tmp)
    os.environ.update(ROBOTUTOR_WORKSPACE=str(root / "personal"),
                      ROBOTUTOR_ASSIGNMENTS=str(root / "assigned"), ROBOTUTOR_PORT="1",
                      ROBOTUTOR_REPLAY_POLICY="history")
    os.environ["ROBOTUTOR_ASTRA_REVIEW"] = "0"
    os.environ["ROBOTUTOR_CODEX"] = str(root / "offline-no-codex")
    from app import GLib, Gtk, Pilot, words

    def wait(predicate):
        limit = time.monotonic() + 12
        while time.monotonic() < limit:
            while GLib.MainContext.default().pending():
                GLib.MainContext.default().iteration(False)
            if predicate():
                return
            time.sleep(.02)
        raise AssertionError("Workspace transition timed out")

    def resize_window(width, height):
        if os.name != "nt":
            app.win.set_default_size(width, height)
            return
        # GTK's default size applies before mapping on Win32. Resize the actual test window.
        import ctypes
        from ctypes import wintypes
        api = ctypes.windll.user32
        callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
        api.SetWindowPos.argtypes = (wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int,
                                      ctypes.c_int, ctypes.c_int, wintypes.UINT)
        api.GetWindowThreadProcessId.argtypes = (wintypes.HWND, ctypes.POINTER(wintypes.DWORD))
        api.IsWindowVisible.argtypes = (wintypes.HWND,)
        api.GetDpiForWindow.argtypes = (wintypes.HWND,)
        def resize(hwnd, _):
            pid = wintypes.DWORD()
            api.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            if pid.value == os.getpid() and api.IsWindowVisible(hwnd):
                scale = api.GetDpiForWindow(hwnd) / 96
                api.SetWindowPos(hwnd, None, 0, 0, round(width * scale), round(height * scale), 0x16)
                return False
            return True
        api.EnumWindows(callback(resize), 0)

    app = Pilot()
    app.register(None)
    app.activate()
    try:
        wait(lambda: app.ready)
        assert app.answer_hint.get_visible() and not app.send_button.get_sensitive()
        # Exercise the same render path used when a journal is reopened.
        prompt = "The wheel turns four times. How many turns follow three pedal turns?"
        answer = "I wrote **x** and <b>y</b>."
        app.chat(answer, "You")
        app.record({"event": "delivered", "text": "# Gear ratios\n\n- Use **four** turns.\n"
                    + "```python\nx = 3 ** 2  # <wheel>\n```\n\n" + prompt,
                    "lesson": {"exercise": prompt}})
        text = words(app.history)
        assert answer in text and "• Use four turns." in text and "x = 3 ** 2  # <wheel>" in text
        assert app.exercise.get_text() == prompt and "times.\nHow many" in words(app.exercise)
        def has_tag(widget, fragment, tag):
            buf = widget.get_buffer()
            it = buf.get_iter_at_offset(words(widget).index(fragment))
            return it.has_tag(buf.get_tag_table().lookup(tag))
        assert has_tag(app.history, "Gear ratios", "heading")
        assert has_tag(app.history, "four turns.", "emphasis")
        assert has_tag(app.history, "x = 3", "code")
        assert has_tag(app.exercise, "How many", "ask")
        app.chat("Good observation. " + prompt, "Luna", prompt)
        assert "Good observation.\n\nThe wheel turns four times.\nHow many" in words(app.history)
        one_line = "Which quantity belongs on the horizontal axis?"
        app.exercise.set_question(one_line)
        assert words(app.exercise) == one_line
        wait(lambda: app.question_scroll.get_max_content_height() ==
             app.exercise.measure(Gtk.Orientation.VERTICAL, app.exercise.get_width()).natural)
        short_question = app.question_scroll.get_height()
        passage = ("Team A covered the tray. Team B removed the lid. " * 48
                   + "\n\nWhich comparison would isolate the effect of the lid?")
        app.exercise.set_question(passage)
        wait(lambda: app.question_scroll.get_height() > short_question)
        wide = app.question_scroll.get_height()
        resize_window(750, 620)
        wait(lambda: app.narrow and short_question <= app.question_scroll.get_height() < wide)
        question_adjustment = app.question_scroll.get_vadjustment()
        assert question_adjustment.get_upper() > question_adjustment.get_page_size()
        question_adjustment.set_value(question_adjustment.get_upper() - question_adjustment.get_page_size())
        def last_line_visible():
            end = app.exercise.get_iter_location(app.exercise.get_buffer().get_end_iter())
            visible = app.exercise.get_visible_rect()
            return end.y + end.height <= visible.y + visible.height
        wait(last_line_visible)
        app.question_panel.set_expanded(False)
        app.exercise.set_question(one_line)
        resize_window(1160, 740)
        wait(lambda: not app.narrow)
        assert not app.question_scroll.get_mapped() and app.send_button.get_mapped()
        app.question_panel.set_expanded(True)
        wait(lambda: app.question_scroll.get_mapped() and app.question_scroll.get_height() == short_question)

        short_height = app.answer_scroll.get_height()
        app.editor.get_buffer().set_text("A line of working.\n" * 12)
        wait(lambda: app.answer_scroll.get_height() > short_height)
        assert not app.answer_hint.get_visible() and app.send_button.get_sensitive()
        app.editor.get_buffer().set_text("An unsent multiline\nanswer.")
        app.stop()
        args = [os.environ["ROBOTUTOR_BIN"], "--workspace", app.folder, "--session", app.path.name]
        def terminal(lines):
            return subprocess.run(args, input=lines, text=True, encoding="utf-8", capture_output=True,
                                  check=True, timeout=15).stdout
        assert "An unsent multiline\nanswer." in terminal("/draft\n/activity graph\n/quit\n")
        app.start()
        wait(lambda: app.ready and app.form)
        assert app.answer_hint.get_text() == "Add a note about your work…"
        app.activity.widgets["axis"].set_active_id("Pedal turns")
        app.activity.widgets["first"].set_text("1,3")
        app.stop()
        partial = terminal("/work\n/field second 2,6\n/quit\n")
        assert "One pedal turn: 1,3" in partial
        app.start()
        wait(lambda: app.ready)
        assert app.activity.widgets["second"].get_text() == "2,6"
        app.activity.widgets["relationship"].set_active(1)
        app.send()
        wait(lambda: app.ready and app.form and app.form["id"] == "words")
        assert "reverses" in app.reason.get_text()
        app.command("/plan")
        wait(lambda: app.ready and app.corrections.get_visible())
        app.correct_source()
        buf = app.editor.get_buffer()
        buf.set_text(words(app.editor) + "I misclicked the sentence. I want to check the axes.")
        app.send()
        wait(lambda: app.ready and "misclicked" in app.exercise.get_text())
        app.stop()
        replay = terminal("/plan\n/quit\n")
        assert "Corrected reading" in replay and "misclicked" in replay
        rows = [json.loads(line) for line in app.path.read_text().splitlines()]
        assert sum(r["event"] == "practice_answer" for r in rows) == 1
        assert not any(r["event"] == "learner" and "An unsent" in r["text"] for r in rows)
        # Reopen a legacy checkpoint with a structured exercise on hold during recall.
        graph = next(r["lesson"] for r in rows if (r.get("lesson", {}).get("activity") or {}).get("id") == "graph")
        graph["recall_items"] = [{"term": "gear ratio", "reference": "wheel turns per pedal turn",
                                 "source": "workshop#1", "state": "asking", "resume": graph["exercise"]}]
        resumed = {"event": "continued", "lesson": graph, "from": {"source": "workshop", "digest": "fixture"}}
        with app.path.open("a") as journal:
            journal.write(json.dumps(resumed) + "\n")
        app.start()
        wait(lambda: app.ready)
        recall_question = "What does “gear ratio” mean here? Explain it in your own words, or say ‘skip’."
        assert app.exercise.get_text() == recall_question and not app.form
        app.stop()
        assert recall_question in terminal("/quit\n")
        print("GTK and terminal preserved unsent text and plotted points, delivered the mixed-evidence branch once, and reopened the learner’s correction.")
    finally:
        app.stop()
        app.win.destroy()
