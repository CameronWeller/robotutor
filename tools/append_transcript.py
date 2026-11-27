#!/usr/bin/env python3
"""Append reviewed UTF-8 Markdown from stdin to the private Robotutor transcript repository."""
import argparse
import hashlib
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "desktop"))
from local_platform import lock_file, quiet_process

BRANCH = "refs/heads/main"
PATH = re.compile(r"logs/(?:coverage\.md|[0-9]{4}/[0-9]{2}/[0-9]{2}/[a-z0-9]+(?:-[a-z0-9]+)+\.md)")

def validate(path, content, entry):
    if not PATH.fullmatch(path) or not re.fullmatch(r"[a-zA-Z0-9_-]{1,100}", entry):
        raise ValueError("Supply an approved transcript or coverage path and a stable entry ID")
    if not content.strip() or len(content) > 512000 or "\0" in content:
        raise ValueError("Expected a nonempty reviewed Markdown entry under 512 KB")
    if "<!-- transcript-entry " in content:
        raise ValueError("Reserved entry marker in input")


def publish(repo, path, content, entry):
    return publish_many(repo, path, [(content, entry)])


def publish_many(repo, path, entries, keep_existing=False, message="Append reviewed development record"):
    """Publish several entries to one record in a single archive commit.

    An entry already present is left as it is. With keep_existing, an entry ID that is present with
    different content is also left as it is instead of raising, so recapture never rewrites a record.
    """
    path = path.removeprefix("logs/")
    blocks = []
    for content, entry in entries:
        digest = hashlib.sha256(content.encode()).hexdigest()
        prefix = f"<!-- transcript-entry {entry} "
        block = (f"{prefix}{digest} -->" + "\n" + content.rstrip("\n") + "\n").encode("utf-8")
        blocks.append((prefix.encode("ascii"), block))
    with tempfile.TemporaryDirectory() as tmp:
        env = {**os.environ, "GIT_INDEX_FILE": str(Path(tmp) / "index")}
        def git(*args, data=None, check=True, binary=False):
            return subprocess.run(
                ["git", "-C", str(repo), *args], input=data,
                encoding=None if binary else "utf-8", capture_output=True,
                check=check, env=env, **quiet_process()
            )
        def fetch_snapshot(ref):
            # Keep the caller’s FETCH_HEAD and remote-tracking refs out of concurrent fetches.
            git("fetch", "--no-tags", "--no-write-fetch-head", "--refmap=", "origin", f"{BRANCH}:{ref}")

        for _ in range(12):
            # Fetch into a unique ref so other writers cannot change the snapshot.
            ref = "refs/transcript-tmp/" + Path(tmp).name
            try:
                fetch_snapshot(ref)
                parent = git("rev-parse",ref).stdout.strip()
                marker_file = git("show", f"{parent}:.archive-format", check=False)
                if marker_file.stdout.strip() != "robotutor-development-transcripts-v1":
                    raise ValueError("The destination is not the transcript archive")
                record = git("ls-tree",parent,"--",path).stdout
                if record and not record.startswith("100644 blob "):
                    raise ValueError("Expected a regular Markdown file")
                old = git("show", f"{parent}:{path}", binary=True).stdout if record else b""
                framed, added = b"\n" + old, b""
                present = set(re.findall(rb"<!-- transcript-entry ([A-Za-z0-9_-]+) ", old))
                seen = set()
                for prefix, block in blocks:
                    ident = prefix[len(b"<!-- transcript-entry "):-1]
                    if ident in present:
                        # Early Windows writers stored whole entries with CRLF.
                        if (keep_existing or b"\n" + block in framed
                                or b"\n" + block.replace(b"\n", b"\r\n") in framed):
                            continue
                        raise ValueError("Entry ID already has different content")
                    if ident in seen:
                        if b"\n" + block in added:
                            continue
                        raise ValueError("Entry ID repeated with different content")
                    seen.add(ident)
                    added += b"\n" + block
                if not added:
                    return parent
                for ancestor in Path(path).parents:
                    if str(ancestor) != ".":
                        obj = git("ls-tree",parent,"--",str(ancestor)).stdout
                        if obj and not obj.startswith("040000 tree "):
                            raise ValueError("Transcript directory is not a Git tree")
                blob = git("hash-object", "-w", "--stdin", data=old + added,
                           binary=True).stdout.decode("ascii").strip()
                git("read-tree",parent)
                git("update-index","--add","--cacheinfo",f"100644,{blob},{path}")
                tree = git("write-tree").stdout.strip()
                commit = git("commit-tree",tree,"-p",parent,"-m",message).stdout.strip()
                push = git("push","origin",f"{commit}:{BRANCH}",check=False)
                if push.returncode == 0:
                    fetch_snapshot(ref)
                    git("merge-base", "--is-ancestor", commit, ref)
                    published = git("show", f"{ref}:{path}", binary=True).stdout
                    if not published.startswith(old + added):
                        raise RuntimeError("Remote transcript verification failed")
                    return commit
                if "rejected" not in push.stderr and "failed to update ref" not in push.stderr:
                    raise RuntimeError(push.stderr)
            finally:
                git("update-ref","-d",ref)
        raise RuntimeError("Concurrent writers kept updating the branch. Retry with the same entry ID.")

def outbox_folder(repo):
    root = subprocess.check_output(
        ["git", "-C", str(repo), "rev-parse", "--show-toplevel"], text=True, **quiet_process()
    ).strip()
    if Path(root).resolve() != Path(repo).resolve() or not (Path(root) / ".archive-format").is_file():
        raise ValueError("Initialize the private archive and use its checkout as --repo")
    common = subprocess.check_output(
        ["git", "-C", str(repo), "rev-parse", "--path-format=absolute", "--git-common-dir"],
        text=True, **quiet_process(),
    ).strip()
    folder = Path(common) / "transcript-outbox"
    folder.mkdir(mode=0o700, exist_ok=True)
    return folder


def outbox(repo):
    db = sqlite3.connect(outbox_folder(repo) / "reviewed.sqlite3", timeout=30)
    db.execute("PRAGMA synchronous=FULL")
    db.execute("CREATE TABLE IF NOT EXISTS pending "
               "(id INTEGER PRIMARY KEY, path TEXT, entry TEXT, content TEXT, "
               "UNIQUE(path, entry, content))")
    return db


def append(repo, path, content, entry):
    validate(path, content, entry)
    db = outbox(repo)
    try:
        with db:
            db.execute("INSERT OR IGNORE INTO pending(path,entry,content) VALUES(?,?,?)",
                       (path, entry, content))
            sequence = db.execute("SELECT id FROM pending WHERE path=? AND entry=? AND content=?",
                                  (path, entry, content)).fetchone()[0]
        # An earlier offline entry must reach the branch before a later reply in the same record.
        lock_name = hashlib.sha256(path.encode()).hexdigest() + ".lock"
        with (outbox_folder(repo) / lock_name).open("a") as lock:
            lock_file(lock)
            pending = db.execute("SELECT id, entry, content FROM pending WHERE path=? AND id<=? ORDER BY id",
                                 (path, sequence)).fetchall()
            for row_id, saved_entry, saved_content in pending:
                commit = publish(repo, path, saved_content, saved_entry)
                with db:
                    db.execute("DELETE FROM pending WHERE id=?", (row_id,))
                if saved_entry == entry and saved_content == content:
                    return commit
            # A concurrent drain already published this entry. Verify the exact text on the remote.
            return publish(repo, path, content, entry)

    finally:
        db.close()


def retry(repo):
    db = outbox(repo)
    try:
        for path, entry, content in db.execute(
                "SELECT path,entry,content FROM pending ORDER BY id").fetchall():
            print(append(repo, path, content, entry))
    finally:
        db.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", nargs="?")
    parser.add_argument("--entry")
    parser.add_argument("--repo", default=str(Path(__file__).resolve().parents[1] / "logs"))
    parser.add_argument("--retry-pending", action="store_true")
    args = parser.parse_args()
    if args.retry_pending and not (args.path or args.entry):
        retry(args.repo)
    elif args.path and args.entry and not args.retry_pending:
        print(append(args.repo, args.path, sys.stdin.read(), args.entry))
    else:
        parser.error("Use PATH and --entry ID, or --retry-pending.")
