#!/usr/bin/python3
"""Exercise the journal reader and linked objects in an isolated fictional lesson."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

from app import GLib, Gtk, Pilot, words


def wait(predicate, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        while GLib.MainContext.default().pending():
            GLib.MainContext.default().iteration(False)
        if predicate():
            return
        time.sleep(.025)
    raise AssertionError("Inspector did not reach the expected state")


def screenshot(window, path):
    # Snapshot this GTK surface alone. PyGObject cannot unmarshal every GskRenderNode subtype.
    paint = Gtk.WidgetPaintable.new(window)
    snapshot = Gtk.Snapshot.new()
    paint.snapshot(snapshot, window.get_width(), window.get_height())
    lib = ctypes.CDLL("libgtk-4.so.1")
    ptr = ctypes.c_void_p
    lib.gtk_snapshot_to_node.argtypes, lib.gtk_snapshot_to_node.restype = [ptr], ptr
    lib.gsk_renderer_render_texture.argtypes, lib.gsk_renderer_render_texture.restype = [ptr, ptr, ptr], ptr
    lib.gdk_texture_save_to_png.argtypes = [ptr, ctypes.c_char_p]
    lib.gsk_render_node_unref.argtypes = [ptr]
    lib.g_object_unref.argtypes = [ptr]
    node = lib.gtk_snapshot_to_node(hash(snapshot))
    assert node
    texture = lib.gsk_renderer_render_texture(hash(window.get_renderer()), node, None)
    try:
        assert texture and lib.gdk_texture_save_to_png(texture, os.fsencode(path))
    finally:
        if texture:
            lib.g_object_unref(texture)
        lib.gsk_render_node_unref(node)


def capture(window, folder, name):
    if folder:
        folder.mkdir(mode=0o700, parents=True, exist_ok=True)
        # Wait for a completed allocation and paint after reorienting the split.
        settled = []
        GLib.timeout_add(450, lambda: settled.append(True) or False)
        wait(lambda: settled)
        painted = []
        window.add_tick_callback(lambda *_: painted.append(True) or len(painted) < 4)
        wait(lambda: len(painted) == 4)
        screenshot(window, folder / name)


def run(screens):
    with tempfile.TemporaryDirectory() as temp:
        base = Path(temp)
        os.environ.update(ROBOTUTOR_TEST_HOME=str(base), ROBOTUTOR_ASTRA_REVIEW="0")
        app = Pilot(test_learner="known")
        app.register(None)
        app.activate()
        inspector = app.inspector
        try:
            wait(lambda: app.ready)
            lesson_window = app.win
            draft = "I’m comparing wheel turns to pedal turns."
            app.editor.get_buffer().set_text(draft)
            app.explorer_button.emit("clicked")
            assert app.pages.get_visible_child_name() == "Lesson"
            assert inspector.get_root() == app.editor.get_root() == lesson_window
            assert app.get_windows() == [lesson_window]
            wait(lambda: inspector.picker.get_active_id() == "3")
            assert app.rail.get_mapped() and app.reason.get_mapped()
            assert len(inspector.records) >= 4
            capture(app.win, screens, "linked-decision.png")
            inspector.jump("workshop#4.1")
            wait(lambda: inspector.evidence["workshop#4.1"].has_focus() or inspector.scroll.get_vadjustment().get_value() > 0)
            inspector.jump("first-visit#2.2")
            wait(lambda: inspector.evidence["first-visit#2.2"].get_expanded())
            inspector.picker.set_active_id("0")
            assert not inspector.follow.get_active()
            # Keep the actual tutor's journal unchanged. Exercise partial writes in a disposable copy.
            clone = base / "trace.jsonl"
            clone.write_bytes(b"".join(line for line in app.path.read_bytes().splitlines(keepends=True)
                                       if json.loads(line)["event"] != "workspace_draft"))
            clone.chmod(0o600)
            inspector.bind(clone)
            wait(lambda: len(inspector.records) == 4)
            inspector.picker.set_active_id("0")
            row = json.dumps({"event": "timing", "recorded_at": int(time.time()),
                              "sample": {"kind": "fixture", "delivered": True, "feedback_us": 50000,
                                         "stages": [{"name": "routing", "micros": 10000},
                                                    {"name": "verification", "micros": 40000, "usage": {"total_tokens": 75}}]}})
            def append(fragment):
                with clone.open("a") as stream:
                    stream.write(fragment)
            append(row[:20])
            wait(lambda: inspector.observed_size == clone.stat().st_size)
            assert len(inspector.records) == 4
            append(row[20:] + "\n")
            wait(lambda: len(inspector.records) == 5)
            assert inspector.picker.get_active_id() == "0"
            inspector.filter.set_active(3)
            assert inspector.picker.get_active_id() == "4"
            capture(app.win, screens, "timing.png")
            app.win.set_default_size(760, 620)
            wait(lambda: app.win.get_width() < 880)
            wait(lambda: app.narrow and inspector.get_mapped() and app.send_button.get_mapped())
            assert words(app.editor) == draft
            capture(app.win, screens, "timing-narrow.png")
            inspector.scroll.get_vadjustment().set_value(inspector.scroll.get_vadjustment().get_upper())
            capture(app.win, screens, "timing-detail-narrow.png")
            assert inspector.scroll.get_vadjustment().get_value() > 0
            app.explorer_button.grab_focus()
            app.explorer_button.emit("clicked")
            wait(lambda: not inspector.get_mapped())
            assert words(app.editor) == draft and app.send_button.get_sensitive()
            app.explorer_button.emit("clicked")
            wait(lambda: inspector.get_mapped())
            assert inspector.picker.get_active_id() == "4"
            app.notes_button.emit("clicked")
            assert app.pages.get_visible_child_name() == "Notes"
            app.navigate("Lesson")
            app.win.set_default_size(1160, 740)
            wait(lambda: not app.narrow and inspector.get_mapped())
            assert app.rail.get_mapped() and app.reason.get_mapped()
            app.split.set_position(app.split.get_position() - 65)
            assert words(app.editor) == draft and app.get_windows() == [lesson_window]
            app.switch_profile("new")
            assert app.explorer_button.get_active()
            wait(lambda: app.ready and inspector.records)
            assert all("workshop#" not in json.dumps(record) for record in inspector.records)
            tool = os.environ["ROBOTUTOR_BIN"]
            one = subprocess.check_output([tool, "--inspect-journal", clone, "0", "1"], text=True)
            assert json.loads(one)["record"]["event"] == "open"
            for args in ([clone, "2"], [base / "alias.jsonl"]):
                if len(args) == 1:
                    args[0].symlink_to(clone)
                assert subprocess.run([tool, "--inspect-journal", *args], capture_output=True).returncode
            clone.chmod(0o644)
            assert subprocess.run([tool, "--inspect-journal", clone], capture_output=True).returncode
        finally:
            app.stop()
            app.win.destroy()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--screenshots", type=Path, help="Save GTK images of fictional fixtures here")
    run(parser.parse_args().screenshots)
    print("Verified linked evidence, correction navigation, timing, partial appends, file boundaries and profile isolation.")
