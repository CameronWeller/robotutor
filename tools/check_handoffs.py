"""Verify cross-repository handoffs in an initialized private archive."""
import json
from pathlib import Path
import subprocess

def verify(root):
    archive = root / "logs"
    def git(repo, *args):
        return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()
    records = json.loads(git(archive, "show", "HEAD:handoff-register.json"))
    seen = set()
    for row in records:
        name = row["session"]
        parent = row["predecessor"]
        if name in seen or (parent is not None and parent not in seen):
            raise ValueError(f"Invalid session or predecessor for {name}")
        work_repository = row.get("work_repository", "code")
        if work_repository not in {"code", "archive"}:
            raise ValueError(f"Unknown work repository for {name}: {work_repository}")
        work_root = archive if work_repository == "archive" else root
        for repo, revision in [(work_root, row["work"]), (archive, row["archive"])]:
            if git(repo, "rev-parse", "--verify", revision + "^{commit}") != revision:
                raise ValueError(f"Expected a full commit ID for {name}")
            git(repo, "merge-base", "--is-ancestor", revision, "HEAD")
        git(archive, "cat-file", "-e", row["archive"] + ":" + row["transcript"])
        seen.add(name)
    print(f"Verified {len(seen)} private handoffs")

if __name__ == "__main__":
    verify(Path(__file__).resolve().parents[1])
