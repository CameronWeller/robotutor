import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import session_archive as archive

THREAD = "00000000-0000-4000-8000-000000000123"
STAMP = "2026-11-23T23:00:00Z"


def row(kind, body, ident, **change):
    item = {"type": kind, "id": ident, "content": [{"type": "Text", "text": body}], **change}
    return json.dumps({"type": "event_msg", "timestamp": STAMP, "payload": {
        "type": "item_completed", "thread_id": THREAD, "item": item}}).encode() + b"\n"


class CaptureTests(unittest.TestCase):
    @unittest.skipUnless(os.name == "nt", "Windows console inheritance")
    def test_windowless_launcher_and_git_grandchild_keep_consoles_hidden(self):
        check = ("import ctypes; w=ctypes.windll.kernel32.GetConsoleWindow(); "
                 "assert not w or not ctypes.windll.user32.IsWindowVisible(w)")
        worker = (check + "; import subprocess,sys; "
                  "subprocess.run(['git','--version'],check=True); "
                  f"subprocess.run([sys.executable,'-c',{check!r}],check=True)")
        launcher = ("import subprocess,sys; from local_platform import quiet_process; "
                    f"r=subprocess.run([{sys.executable!r},'-c',{worker!r}],"
                    "stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,**quiet_process()); "
                    "sys.exit(r.returncode)")
        subprocess.run([Path(sys.executable).with_name("pythonw.exe"), "-c", launcher],
                       cwd=Path(__file__).resolve().parents[1] / "desktop", check=True,
                       timeout=10, **archive.quiet_process())

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.folder = Path(tmp.name)
        self.source = self.folder / "rollout.jsonl"
        self.db = archive.connect(self.folder)
        self.addCleanup(self.db.close)
        self.tag = archive.tag(THREAD)
        self.path = f"logs/2026/11/23/codex-session-{self.tag}.md"
        with self.db:
            self.db.execute("INSERT INTO sessions(id,tag,source,archive) VALUES (?,?,?,?)",
                            (THREAD, self.tag, str(self.source), self.path))

    def session(self):
        return self.db.execute("SELECT * FROM sessions").fetchone()

    def test_capture_restart_and_partial_write(self):
        first = row("UserMessage", "private wording", "1")
        last = row("AgentMessage", "visible progress", "2", phase="commentary")
        self.source.write_bytes(first + row("Reasoning", "hidden", "3") +
                                row("McpToolCall", "private tool result", "4") + last[:-1])
        archive.ingest(self.db, self.session(), 0)
        self.assertEqual(self.db.execute("SELECT count(*) FROM messages").fetchone()[0], 1)
        with self.source.open("ab") as f:
            f.write(b"\n" + last)
        archive.ingest(self.db, self.session(), 0)
        self.assertEqual([r[0] for r in self.db.execute("SELECT body FROM messages ORDER BY seq")],
                         ["private wording", "visible progress"])
        self.assertIsNone(archive.visible(json.loads(first), "another thread", 0))
        self.assertIsNone(archive.visible(json.loads(first), THREAD, 2e9))

    def test_publisher_retry_keeps_frozen_batch_and_no_dialogue(self):
        self.source.write_bytes(row("UserMessage", "private wording", "1"))
        archive.ingest(self.db, self.session(), 0)
        with patch.object(archive, "append", side_effect=[1, 2, OSError()]) as broken:
            with self.assertRaises(OSError):
                archive.publish_batch(".", self.db, self.session())
            interrupted = broken.call_args.args
        with self.source.open("ab") as f:
            f.write(row("AgentMessage", "private reply", "2", phase="final_answer"))
        archive.ingest(self.db, self.session(), 0)
        with patch.object(archive, "append") as publish:
            archive.publish_batch(".", self.db, self.session())
            self.assertEqual(publish.call_args.args, interrupted)
            archive.publish_batch(".", self.db, self.session())
            self.assertEqual(publish.call_count, 4)
            self.assertNotIn("private", str(publish.call_args_list))
        self.assertEqual(self.db.execute("SELECT count(*) FROM messages WHERE sent=0").fetchone()[0], 0)

    def test_git_hook_links_once_and_skips_human_commits(self):
        message = self.folder / "message"
        message.write_text("Implement lesson review\n")
        with patch.dict("os.environ", {"CODEX_THREAD_ID": ""}):
            archive.trailer(self.folder, message, "message")
        self.assertNotIn("Session", message.read_text())
        with patch.dict("os.environ", {"CODEX_THREAD_ID": THREAD}):
            archive.trailer(self.folder, message, "message")
            archive.trailer(self.folder, message, "message")
        parsed = subprocess.check_output(["git","interpret-trailers","--parse",str(message)],text=True)
        self.assertEqual(parsed.strip(), "Session-Log: " + archive.ARCHIVE + self.path[5:])


if __name__ == "__main__":
    unittest.main()
