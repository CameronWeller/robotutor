"""Exercise persistence and conflicting edits using two clients in a fictional workspace."""
import tempfile
import unittest
from pathlib import Path

from sustained import AssignmentStore, planning_step


class SustainedAssignmentTest(unittest.TestCase):
    def test_partial_plan_draft_coaching_and_archives(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            store = AssignmentStore(folder)
            key = store.open()
            def turn(action, value=""):
                store.turn(store.read(key), action, value)
            turn("answer", "Compare options")
            self.assertIn("options", planning_step(store.read(key)["brief"])[0])
            store = AssignmentStore(folder)
            self.assertEqual(store.open(), key)
            with self.assertRaises(ValueError):
                turn("accept")
            for answer in ("Two bridge designs", "A report", "I explain one trade-off", "Gather dimensions"):
                turn("answer", answer)
            turn("revise", "deliverable")
            turn("answer", "An illustrated guide")
            self.assertEqual(store.read(key)["brief"]["checkpoint"], "Gather dimensions")
            turn("accept")
            with self.assertRaises(ValueError):
                turn("finish")
            store.save_draft(key, "", "My comparison")
            stale = store.read(key)
            peer = AssignmentStore(folder)
            peer.save_draft(key, "My comparison", "My revised comparison")
            with self.assertRaises(ValueError):
                store.save_draft(key, "My comparison", "An earlier draft")
            with self.assertRaises(ValueError):
                store.turn(stale, "finish")
            turn("coach", "Check my evidence")
            self.assertIn("trade-off", store.read(key)["turns"][-1][1])
            reloaded = peer.read(peer.open())
            self.assertEqual(reloaded["coach"], "Check my evidence")
            store.turn(reloaded, "reply", "Bridge A uses less steel.")
            with self.assertRaises(ValueError):
                store.turn(reloaded, "reply", "Duplicate turn")
            self.assertEqual(store.read(key)["draft"], "My revised comparison")
            turn("checkpoint", "Compare load limits")
            turn("finish")
            self.assertEqual(peer.finished(), [(key, "Two bridge designs")])
            self.assertNotEqual(peer.open(), key)
            self.assertEqual(peer.read(key)["checkpoint"], "Compare load limits")


if __name__ == "__main__":
    unittest.main()

