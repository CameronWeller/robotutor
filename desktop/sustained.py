"""Private, resumable learner assignments. Each planning or coaching turn is atomic."""
import json
import sqlite3
from contextlib import closing
from pathlib import Path

FIELDS = ("purpose", "topic", "deliverable", "criterion", "checkpoint")
LABELS = ("Purpose", "Topic", "Deliverable", "Success check", "First checkpoint")
GOALS = ("Explain an idea", "Compare options", "Support a claim")
COACHES = ("Find a starting point", "Check my evidence", "Revise a passage")


def planning_step(brief):
    field = next((key for key in FIELDS if not brief.get(key)), None)
    purpose = brief.get("purpose")
    if field == "purpose":
        return "What would you like to practice over a few sessions?", GOALS
    if field == "topic":
        return {GOALS[0]: "Which idea would you like to explain?",
                GOALS[1]: "Which options would you like to compare?",
                GOALS[2]: "What claim would you like to test?"}.get(purpose, "What topic shall we use?"), ()
    if field == "deliverable":
        return f"What would you like to make about {brief['topic']}?", ("An article", "An illustrated guide", "A report")
    if field == "criterion":
        options = {GOALS[0]: ("A reader can explain it back", "My example shows how it works"),
                   GOALS[1]: ("Both options use the same test", "I explain one trade-off"),
                   GOALS[2]: ("Evidence supports the claim", "I address a counterexample")}
        return "What will tell you that your work succeeded?", options.get(purpose, ("A reader can use it", "I can justify my decisions"))
    if field == "checkpoint":
        options = {GOALS[0]: ("Collect one example", "Sketch an outline", "Draft the opening"),
                   GOALS[1]: ("Pick a comparison test", "Collect one data point", "Sketch a table"),
                   GOALS[2]: ("Find one source", "List a counterexample", "Draft the claim")}
        return "What is one piece you can finish in your first sitting?", options.get(purpose, ("Sketch an outline", "Collect one example"))
    return "Does this plan feel worth spending time on?", ()


def reason_for_assignment(brief):
    return {
        GOALS[0]: "Draft an explanation now, then revisit it with an example a reader can test.",
        GOALS[1]: "Collect evidence for each option, then revisit your comparison using the same test.",
        GOALS[2]: "Develop your claim, then return to test it against evidence and a counterexample.",
    }.get(brief.get("purpose"), f"Use a first draft to practice “{brief.get('purpose', '')}”, then revise it against your success check.")


def brief_text(brief):
    return "\n".join(f"{label} · {brief[key]}" for key, label in zip(FIELDS, LABELS) if brief.get(key))


class AssignmentStore:
    def __init__(self, folder):
        self.path = Path(folder) / "long-form-assignments.sqlite3"
        with closing(self.connect()) as db, db:
            db.executescript("""
                CREATE TABLE IF NOT EXISTS assignments (
                    id INTEGER PRIMARY KEY, status TEXT NOT NULL DEFAULT 'planning',
                    brief TEXT NOT NULL DEFAULT '{}', draft TEXT NOT NULL DEFAULT '',
                    version INTEGER NOT NULL DEFAULT 0, coach TEXT NOT NULL DEFAULT '',
                    checkpoint TEXT NOT NULL DEFAULT '');
                CREATE UNIQUE INDEX IF NOT EXISTS one_open_assignment
                    ON assignments ((1)) WHERE status != 'finished';
                CREATE TABLE IF NOT EXISTS turns (
                    id INTEGER PRIMARY KEY, assignment_id INTEGER NOT NULL REFERENCES assignments(id),
                    learner TEXT NOT NULL, helper TEXT NOT NULL);
            """)

    def connect(self):
        db = sqlite3.connect(self.path, timeout=2)
        db.row_factory = sqlite3.Row
        return db

    def open(self):
        with closing(self.connect()) as db, db:
            db.execute("INSERT OR IGNORE INTO assignments(status) VALUES ('planning')")
            return db.execute("SELECT id FROM assignments WHERE status != 'finished'").fetchone()[0]

    def finished(self):
        with closing(self.connect()) as db:
            return [(r["id"], json.loads(r["brief"])["topic"]) for r in db.execute(
                "SELECT id,brief FROM assignments WHERE status='finished' ORDER BY id DESC")]

    def read(self, key):
        with closing(self.connect()) as db, db:
            db.execute("BEGIN")
            row = dict(db.execute("SELECT * FROM assignments WHERE id=?", (key,)).fetchone())
            row["brief"] = json.loads(row["brief"])
            row["turns"] = [tuple(r) for r in db.execute(
                "SELECT learner,helper FROM turns WHERE assignment_id=? ORDER BY id", (key,))]
            return row

    def save_draft(self, key, before, after):
        with closing(self.connect()) as db, db:
            result = db.execute("UPDATE assignments SET draft=? WHERE id=? AND status='working' AND draft=?",
                                (after, key, before))
            if not result.rowcount:
                raise ValueError("The saved draft changed in another window. Your text is still here to copy.")

    def turn(self, record, action, answer=""):
        brief = dict(record["brief"])
        status, coach, checkpoint = record["status"], record["coach"], record["checkpoint"]
        value = answer.strip()
        if action in {"answer", "checkpoint", "coach", "reply"} and not value:
            raise ValueError("Enter a few words to continue.")
        if status == "planning":
            field = next((f for f in FIELDS if not brief.get(f)), None)
            if action == "answer" and field:
                brief[field] = value
            elif action == "back" and brief:
                del brief[next(f for f in reversed(FIELDS) if f in brief)]
                value = "Change my last answer"
            elif action == "revise" and value in brief:
                del brief[value]
                value = "Change " + value
            elif action == "accept" and not field:
                status, checkpoint = "working", brief["checkpoint"]
                value = "Start this assignment"
            else:
                raise ValueError("Review the brief before you start.")
            response = planning_step(brief)[0] if status == "planning" else f"Your first checkpoint is “{checkpoint}”."
        elif status == "working":
            if action == "coach" and value in COACHES:
                coach = value
                if value == COACHES[0]:
                    response = f"For “{checkpoint}”, what is the smallest piece you could put on the page?"
                elif value == COACHES[1]:
                    response = f"Your success check is “{brief['criterion']}”. Which example in your draft could help a reader see it?"
                else:
                    response = "Which sentence or passage would you like to change? Paste just that part."
            elif action == "reply" and coach in COACHES:
                excerpt = value if len(value) <= 140 else value[:139] + "…"
                if coach == COACHES[0]:
                    response = f"Try “{excerpt}” as a rough first piece. When you reread it, check it against “{brief['criterion']}”."
                elif coach == COACHES[1]:
                    response = f"Beside “{excerpt}”, add one sentence explaining how the example supports your point about {brief['topic']}."
                else:
                    response = f"Read “{excerpt}” aloud. Try a second version that puts the main idea first, then compare them against “{brief['criterion']}”."
                coach = ""
            elif action == "checkpoint":
                response = f"You set “{value}” as the next checkpoint after “{checkpoint}”."
                checkpoint, coach = value, ""
            elif action == "finish" and record["draft"].strip():
                value, response, status, coach = "Finish this assignment", "Your finished draft and helper turns are saved here.", "finished", ""
            else:
                raise ValueError("Choose a helper move or save your draft before finishing.")
        else:
            raise ValueError("This assignment is complete.")
        with closing(self.connect()) as db, db:
            changed = db.execute("""UPDATE assignments SET status=?, brief=?, coach=?, checkpoint=?, version=version+1
                               WHERE id=? AND version=? AND (? != 'finish' OR draft=?)""",
                                 (status, json.dumps(brief), coach, checkpoint, record["id"], record["version"],
                                  action, record["draft"]))
            if not changed.rowcount:
                raise ValueError("This assignment changed in another window. Reopen it to see the latest version.")
            db.execute("INSERT INTO turns(assignment_id,learner,helper) VALUES (?,?,?)",
                       (record["id"], value, response))

