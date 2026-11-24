"""Exercise release, repair, and personal activity continuity using fictional courses."""
import copy
import json
import os
from pathlib import Path
import tempfile

from course_agent import propose
from personal_courses import open_activity

course = {"title": "Cooking with fractions", "destination": "Scale a recipe",
          "units": [{"title": "Measures", "activities": [
              {"key": "halves", "title": "Compare portions", "objective": "Compare halves",
               "criterion": "Explain why the portions match.", "instructions": "Compare two half cups.", "due": None},
              {"key": "equivalent", "title": "Equivalent measures", "objective": "Rename a fraction",
               "criterion": "Show two equal measures.", "instructions": "Fill one cup using quarters.", "due": None}]}]}
context = {"direction": ["Teach fractions using cooking."]}
for outcomes in ([True], [False, True], [False, False]):
    reviews = iter(outcomes)
    repairs = []
    def generate(prompt, schema):
        if "accepted" in schema["properties"]:
            return {"accepted": next(reviews), "explanation": "Missing a recipe comparison."}
        repairs.append(prompt)
        return {"outline": copy.deepcopy(course), "changes": "Compare portions before renaming them."}
    result = propose(context, generate)
    assert len(repairs) == len(outcomes) and bool(result["problem"]) != outcomes[-1]
    if len(repairs) == 2:
        assert "Missing a recipe comparison." in repairs[-1]

with tempfile.TemporaryDirectory() as tmp:
    folder = Path(tmp)
    activity = course["units"][0]["activities"][-1]
    path = folder / open_activity(folder, "fractions", course, activity)
    previous = path.read_bytes()
    assert path.stat().st_mode & 0o077 == 0
    with path.open("a") as f:
        f.write(json.dumps({"event": "workspace_draft", "turn": 1, "draft": {"text": "Four quarters"}}) + "\n")
    activity["instructions"] = "Use the new recipe."
    assert open_activity(folder, "fractions", course, activity) == path.name
    assert path.read_bytes().startswith(previous) and b"Four quarters" in path.read_bytes()
    import subprocess
    run = subprocess.run([os.environ["ROBOTUTOR_BIN"], "--workspace", folder, "--session", path.name],
                         input="/draft\n/quit\n", capture_output=True, text=True, timeout=20,
                         env=dict(os.environ, ROBOTUTOR_REPLAY_POLICY="history"))
    assert run.returncode == 0 and "Four quarters" in run.stdout, run.stderr
print("Verified independent review, one targeted repair, retained problems and private draft continuity.")
