"""Identify development transcripts requiring review against the implementation register."""
import argparse
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
INDEX = ROOT / "docs/foundation/transcript-sources.json"


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], text=True)


def inventory(repo, refs):
    found = {}
    for ref in sorted(set(refs)):
        rows = git(repo, "ls-tree", "-r", ref, "--", "logs", "docs/foundation/conversations")
        for row in rows.splitlines():
            meta, path = row.split("\t", 1)
            if path in ("logs/coverage.md", "logs/README.md"):
                continue
            found.setdefault(path, set()).add(meta.split()[2])
    return found


def check(name, repo, baseline, snapshot):
    if snapshot:
        refs = baseline["refs"].values()
    else:
        namespace = "refs/remotes/origin" if name == "published" else "refs/heads"
        refs = git(repo, "for-each-ref", "--format=%(objectname)", namespace).splitlines()
    if not refs:
        raise SystemExit(f"No {name} refs available. Fetch the project branches first.")

    current = inventory(repo, refs)
    reviewed = {path: set(record["blobs"]) for path, record in baseline["records"].items()}
    changes = 0
    for path in sorted(current.keys() | reviewed.keys()):
        actual = current.get(path, set())
        expected = reviewed.get(path, set())
        new = actual - expected
        if not actual:
            detail = "review source missing"
        elif new:
            detail = "new blobs " + " ".join(sorted(new))
        elif snapshot and expected != actual:
            detail = "recorded blob missing"
        else:
            continue
        print(name, path, detail)
        changes += 1
    print(f"{name}: {len(current)} paths, {changes} requiring reconciliation")
    return changes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--study", type=Path, help="also inspect the separate simulation checkout")
    parser.add_argument("--snapshot", action="store_true", help="verify the frozen reference set")
    args = parser.parse_args()
    index = json.loads(INDEX.read_text())
    changes = check("published", ROOT, index["published"], args.snapshot)
    if args.study is not None:
        changes += check("study", args.study, index["study"], args.snapshot)
    else:
        print("Separate study checkout was not checked (supply --study).")
    return int(changes != 0)


if __name__ == "__main__":
    raise SystemExit(main())
