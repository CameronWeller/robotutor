"""List private transcript revisions awaiting review, using the archived inventory."""
import argparse
import json
from pathlib import Path
import subprocess

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1] / "logs")
    args = parser.parse_args()
    def git(*parts):
        return subprocess.check_output(["git", "-C", str(args.repo), *parts], text=True)
    base = json.loads(git("show", "HEAD:reviewed-revisions.json"))
    current = {}
    for line in git("ls-tree", "-r", "HEAD").splitlines():
        meta, filename = line.split("\t", 1)
        if filename.startswith("20") and filename.endswith(".md"):
            current[filename] = meta.split()[2]
    changes = sorted(p for p in base.keys() | current.keys() if base.get(p) != current.get(p))
    for filename in changes:
        print(filename, current.get(filename, "missing"))
    print(f"{len(current)} transcripts, {len(changes)} revisions awaiting review")
    return bool(changes)

if __name__ == "__main__":
    raise SystemExit(main())
