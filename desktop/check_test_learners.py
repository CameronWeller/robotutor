#!/usr/bin/python3
"""Exercise test profiles, desktop reset and private trailing review recovery offline."""
import hashlib
import json
import os
from pathlib import Path
import tempfile
import time
from unittest.mock import patch

from app import GLib, Gtk, Pilot, words
import astra_panel
import astra
from test_learners import workspace


def wait(predicate):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        while GLib.MainContext.default().pending():
            GLib.MainContext.default().iteration(False)
        if predicate():
            return
        time.sleep(.02)
    raise AssertionError("Desktop test learner did not become ready")


def profiles(root):
    os.environ.update(ROBOTUTOR_TEST_HOME=str(root), ROBOTUTOR_ASTRA_REVIEW="0")
    tutor = os.environ["ROBOTUTOR_BIN"]
    returning = workspace(root, "returning", tutor)
    assert sorted(p.stem for p in (returning / "personal").glob("*.jsonl")) == ["current", "first-visit"]
    for name in ("known", "new", "returning"):
        app = Pilot(test_learner=name)
        app.set_application_id("org.robotutor.ProfileCheck." + name.capitalize() + str(os.getpid()))
        app.register(None)
        app.activate()
        try:
            wait(lambda: app.ready)
            assert root in app.folder.parents and not app.assignment and not app.locked
            if name == "known":
                assert "5 pedal turns" in app.exercise.get_text()
                app.command("/plan")
                wait(lambda: app.ready and "workshop" in words(app.plan_view))
            elif name == "new":
                assert "something you know well" in words(app.history)
            else:
                assert "repair" in words(app.history).lower()
            original = app.folder
            app.editor.get_buffer().set_text("Synthetic draft before reset")
            window = app.win
            app.switch_profile("new" if name != "new" else "known")
            wait(lambda: app.ready)
            assert app.win is window and app.folder != original and len(app.get_windows()) == 1
            app.show_page("Astra reviews")
            assert app.pages.get_visible_child_name() == "Astra reviews" and app.reviews.folder == app.folder / ".astra"
            app.switch_profile(name)
            wait(lambda: app.ready and "Synthetic draft before reset" in words(app.editor))
            app.reset_test()
            wait(lambda: app.ready)
            assert app.folder != original and original.is_dir() and words(app.editor) == ""
            assert original.parent != workspace(root, name, tutor)
            if name != "returning":
                assert returning == workspace(root, "returning", tutor)
        finally:
            app.stop()
            app.win.destroy()


def reviews(root):
    root.mkdir(mode=0o700)
    packet = json.dumps({"messages": [{"role": "user", "content": "Fictional request"}],
                         "response_format": {"type": "json_object"}})
    digest = hashlib.sha256(packet.encode()).hexdigest()
    through = Path(__file__).resolve().parents[1] / "lab/src/openrouter/trailing.sql"
    with astra.connect(root) as db:
        db.executescript(through.read_text())
        db.execute("INSERT INTO reviews(receipt,session,stage,digest,packet,luna,model) VALUES(?,?,?,?,?,?,?)",
                   ("test", "session", "review", digest, packet, '{"reply":"LUNA_ONLY"}', "luna"))
    prompts = []
    def answer(prompt, schema):
        prompts.append(prompt)
        if len(prompts) == 1:
            assert prompt.endswith(packet) and "LUNA_ONLY" not in prompt
            return {"response_text": '{"reply":"independent answer"}'}
        assert "LUNA_ONLY" in prompt and "independent answer" in prompt
        return {"improvement": True, "observation": "one decision", "luna_change": "request one answer",
                "replay_check": "count the required answers", "luna_evidence": "LUNA_ONLY",
                "astra_evidence": "invented quote" if len(prompts) == 2 else "independent answer"}
    with patch.object(astra, "run_codex", answer), astra.connect(root) as db:
        astra.drain(root)
        row = db.execute("SELECT * FROM reviews").fetchone()
        assert row["status"] == "blocked" and row["candidate"] and row["analysis"] is None
        with db:
            db.execute("UPDATE reviews SET status='pending'")
        astra.drain(root)
        astra.drain(root)
        row = db.execute("SELECT * FROM reviews").fetchone()
        assert row["status"] == "note" and json.loads(row["analysis"])["luna_change"]
        assert len(prompts) == 3
        panel = astra_panel.Reviews()
        panel.bind(root)
        assert panel.summary.get_text().startswith("1 open note")
        for expected in ("Reopen note", "Mark reviewed"):
            panel.toggle.emit("clicked")
            assert panel.toggle.get_label() == expected
        panel.bind(root.parent / "another-learner")
        assert not panel.records and not panel.toggle.get_visible()



if __name__ == "__main__":
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        profiles(root / "scenarios")
        reviews(root / "supervision")
    print("Three GTK profiles resumed and reset independently. Blind replay, grounded notes and blocked-review recovery passed.")
