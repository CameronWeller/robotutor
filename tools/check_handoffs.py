#!/usr/bin/env python3
"""Verify the checkout's handoff and pickup ledgers against local Git history."""
import functools
import os
from pathlib import Path
import re
import subprocess
import sys
from datetime import datetime

HANDOFF = ["Closed (UTC)", "Session", "Earlier handoff", "Dated handoff", "Final hash"]
PICKUP = ["Time (UTC)", "Receiving session", "Event", "Source session",
          "Source final hash", "Work hash", "Activity"]
HISTORY_HINT = "Fetch the missing branch or history (deepen a shallow clone), then retry."


def git(*args):
    # Verification uses local objects and the original commit contents.
    result = subprocess.run(["git", "--no-replace-objects", *args], capture_output=True,
                            env={**os.environ, "GIT_NO_LAZY_FETCH": "1"})
    if result.returncode:
        detail = result.stderr.decode(errors="replace").strip()
        raise ValueError(f"git {args[0]} failed: {detail or result.returncode}")
    return result.stdout


def rows(path, columns):
    """Read the single pipe table. Prose and fenced examples are not ledger entries."""
    table = []
    fence = None
    for n, line in enumerate(Path(path).read_text(encoding="utf-8").splitlines(), 1):
        line = line.strip()
        if fence:
            if re.fullmatch(re.escape(fence[0]) + "{" + str(len(fence)) + ",}", line):
                fence = None
            continue
        marker = re.match(r"`{3,}|~{3,}", line)
        if marker:
            fence = marker[0]
            continue
        if line.startswith("|"):
            if table and n != table[-1][0] + 1:
                raise ValueError(f"{path}:{n}: expected a single contiguous table")
            if not line.endswith("|"):
                raise ValueError(f"{path}:{n}: missing closing pipe")
            cells = [c.strip() for c in re.split(r"(?<!\\)\|", line[1:-1])]
            if len(cells) != len(columns) or not all(cells):
                raise ValueError(f"{path}:{n}: expected {len(columns)} nonempty cells")
            table.append((n, cells))
    if (len(table) < 2 or table[0][1] != columns
            or not all(re.fullmatch(r":?-{3,}:?", s) for s in table[1][1])):
        raise ValueError(f"{path}: expected the ledger header and separator")
    return table[2:]


def stamp_and_session(stamp, session):
    if not re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z", stamp):
        raise ValueError(f"invalid UTC timestamp {stamp!r}")
    datetime.strptime(stamp, "%Y-%m-%dT%H:%M:%SZ")
    if not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", session):
        raise ValueError(f"invalid session slug {session!r}")


def verify():
    os.chdir(os.fsdecode(git("rev-parse", "--show-toplevel").rstrip(b"\n")))
    format_name = git("rev-parse", "--show-object-format").decode().strip()
    width = {"sha1": 40, "sha256": 64}.get(format_name)
    if width is None:
        raise ValueError(f"unsupported Git object format {format_name!r}")

    @functools.lru_cache(maxsize=None)
    def commit(oid):
        if not re.fullmatch("[0-9a-f]{" + str(width) + "}", oid):
            raise ValueError(f"expected a full lowercase {format_name} commit hash, got {oid!r}")
        try:
            kind = git("cat-file", "-t", oid).strip()
        except ValueError as exc:
            raise ValueError(f"missing commit {oid}. {HISTORY_HINT}") from exc
        if kind != b"commit":
            raise ValueError(f"{oid} names a {kind.decode()}, expected a commit")
        refs = git("for-each-ref", "--count=1", "--contains=" + oid, "--format=%(refname)",
                   "refs/heads", "refs/remotes", "refs/tags")
        if not refs:
            # Include detached HEAD without requiring active work to have a branch.
            try:
                git("merge-base", "--is-ancestor", oid, "HEAD")
            except ValueError as exc:
                raise ValueError(f"{oid} is unreachable from local refs and HEAD. {HISTORY_HINT}") from exc

    registered, archives, incoming = {}, set(), []
    for n, (stamp, session, parent, link, oid) in rows("HANDOFF.md", HANDOFF):
        try:
            stamp_and_session(stamp, session)
            commit(oid)
            if oid in registered:
                raise ValueError(f"duplicate handoff final hash {oid}")
            if (not registered and parent != "—") or (registered and parent not in registered):
                raise ValueError("expected one genesis, then a previously registered parent hash")
            path = Path("handoffs", stamp.replace(":", "") + "-" + session, "HANDOFF.md")
            if link != f"[HANDOFF.md]({path.as_posix()})":
                raise ValueError(f"expected dated link to {path}")
            if path in archives:
                raise ValueError(f"duplicate dated namespace {path.parent}")
            if any(p.is_symlink() for p in (path, *path.parents)) or not path.is_file():
                raise ValueError(f"missing regular dated file {path}")
            try:
                entry = git("ls-tree", oid, "--", path.as_posix())
                if not entry.startswith((b"100644 blob ", b"100755 blob ")):
                    raise ValueError(f"{path} is absent or not a regular file at {oid}")
                blob = git("cat-file", "blob", f"{oid}:{path.as_posix()}")
            except ValueError as exc:
                raise ValueError(f"{exc}. {HISTORY_HINT}") from exc
            if path.read_bytes() != blob:
                raise ValueError(f"dated snapshot {path} differs from {oid}")
            # Read the commit object: revision walking hides parents at a shallow boundary.
            headers = git("cat-file", "commit", oid).split(b"\n\n", 1)[0]
            parents = [p[7:].decode("ascii") for p in headers.splitlines() if p.startswith(b"parent ")]
            for parent_oid in parents:
                try:
                    inherited = git("ls-tree", parent_oid, "--", path.as_posix())
                except ValueError as exc:
                    raise ValueError(f"cannot check parent {parent_oid} of {oid}. {HISTORY_HINT}") from exc
                if inherited:
                    raise ValueError(f"{oid} did not first add {path}; it exists in parent {parent_oid}")
            archives.add(path)
            registered[oid] = session
            if parent != "—":
                incoming.append((n, stamp, session, parent))
        except (ValueError, OSError) as exc:
            raise ValueError(f"HANDOFF.md:{n}: {exc}") from exc

    extra = set(Path("handoffs").rglob("HANDOFF.md")) - archives
    if extra:
        raise ValueError("unregistered dated file(s): " + ", ".join(map(str, sorted(extra))))

    pickups = {}
    events = rows("PICKUP.md", PICKUP)
    for n, (stamp, receiver, event, source, oid, work, _activity) in events:
        try:
            stamp_and_session(stamp, receiver)
            if registered.get(oid) != source:
                raise ValueError(f"unknown source session and final hash: {source} {oid}")
            commit(work)
            key = (receiver, oid)
            if event == "picked-up":
                if key in pickups:
                    raise ValueError(f"duplicate pickup by {receiver} from {source}")
                pickups[key] = (stamp, stamp)
            elif event == "worked-on":
                if key not in pickups:
                    raise ValueError(f"worked-on by {receiver} needs a preceding pickup from {source}")
                first, last = pickups[key]
                if stamp < last:
                    raise ValueError(f"events for {receiver} from {source} have decreasing UTC times")
                pickups[key] = (first, stamp)
            else:
                raise ValueError(f"unknown event {event!r}")
        except (ValueError, OSError) as exc:
            raise ValueError(f"PICKUP.md:{n}: {exc}") from exc
    for n, closed, session, parent in incoming:
        receipt = pickups.get((session, parent))
        if receipt is None or receipt[0] > closed:
            raise ValueError(f"HANDOFF.md:{n}: {session} needs a pickup from {registered[parent]} "
                             f"at {parent} by its close time")
    print(f"Verified {len(archives)} dated handoff(s) and {len(events)} pickup/work event(s).")


if __name__ == "__main__":
    try:
        verify()
    except (ValueError, OSError) as error:
        sys.exit(str(error))
