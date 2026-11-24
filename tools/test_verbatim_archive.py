import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch

import session_archive as archive
import verbatim_archive as v
from test_session_archive import THREAD, STAMP, row


def line(kind, payload):
    return json.dumps({"type": kind, "timestamp": STAMP, "payload": payload}).encode() + b"\n"


@unittest.skipUnless(shutil.which("age") and shutil.which("age-keygen"), "age executables required")
class VerbatimTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        self.db = archive.connect(root / "private")
        self.addCleanup(self.db.close)
        self.key,self.recipient = v.identity(root / "private", self.db)
        self.source = root / "journal"
        self.tag = archive.tag(THREAD)
        self.session = dict(id=THREAD, tag=self.tag, source=str(self.source),
                            archive="logs/2026/11/23/codex-session-" + self.tag + ".md")

    def test_exact_history_choices_tool_privacy_and_duplicate_occurrences(self):
        user = "  I meant every word.\r\n\tCafé 👩🏽‍💻  "
        q = {"questions":[{"title":"What exactly?","options":["all words","tool summaries"]}]}
        answer = {"answers":{"q":{"answers":["all words "]}}}
        begin = line("event_msg", {"type":"task_started","turn_id":"t"})
        ask = line("response_item", {"type":"function_call","name":"request_user_input","call_id":"c",
                                     "arguments":json.dumps(q)})
        reply = line("response_item", {"type":"function_call_output","call_id":"c","output":json.dumps(answer)})
        copy = line("response_item", {"type":"message","role":"assistant","phase":"final_answer",
                                      "content":[{"type":"output_text","text":" yes "}]})
        hidden = row("Reasoning","sensitive hidden analysis","r")
        tool = row("CommandExecution","sensitive tool output","tool",status="completed")
        self.source.write_bytes(begin + row("UserMessage",user,"u") + row("UserMessage",user,"v") +
                                ask + reply + hidden + tool + copy + row("AgentMessage"," yes ","a"))
        v.ingest_history(self.db,self.session)
        v.ingest_history(self.db,self.session)
        commits = {}
        def sink(repo,path,body,entry):
            commits[(path,entry)] = body
        with patch.object(v,"append",side_effect=sink):
            v.publish_encrypted(".",self.db,self.session,self.recipient)
        encrypted = "".join(value for (path,_),value in commits.items() if path.endswith("-verbatim.md"))
        self.assertNotIn("every word",encrypted)
        events = v.recover(encrypted,self.tag,self.key)
        rendered = list(v.dialogue(events))
        self.assertEqual([e["parts"] for e in rendered if e["role"]=="user"],[[user],[user]])
        self.assertIn(q,[e.get("control") for e in rendered])
        self.assertIn(answer["answers"],[e.get("answers") for e in rendered])
        self.assertEqual(sum(e["role"]=="assistant" for e in rendered),1)
        self.assertNotIn("sensitive",json.dumps(events))
        self.assertEqual(len([e for e in rendered if e["role"]=="tool"]),1)
        self.assertEqual(self.db.execute("SELECT sum(1-backed) FROM conversation_events").fetchone()[0],0)

    def test_large_message_interrupted_publication_and_wrong_identity(self):
        text = os.urandom(350000).hex()
        self.source.write_bytes(row("UserMessage",text,"u"))
        v.ingest_history(self.db,self.session)
        with patch.object(v,"append",side_effect=OSError):
            with self.assertRaises(OSError):
                v.publish_encrypted(".",self.db,self.session,self.recipient)
        chunks = [r[0] for r in self.db.execute("SELECT armor FROM sealed_chunks ORDER BY part")]
        self.assertGreater(len(chunks),1)
        with self.assertRaises(ValueError):
            v.recover(chunks[0],self.tag,self.key)
        with patch.object(v,"append") as appended:
            v.publish_encrypted(".",self.db,self.session,self.recipient)
        self.assertEqual(v.recover("".join(chunks),self.tag,self.key)[0]["parts"],[text])
        self.assertIn(chunks[0].strip(),appended.call_args_list[0].args[2])
        self.key.unlink()
        with self.assertRaises(RuntimeError):
            v.identity(self.key.parent.parent / "private",self.db)


if __name__ == "__main__":
    unittest.main()
