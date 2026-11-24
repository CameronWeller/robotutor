#!/usr/bin/env python3
"""Read one named Codex development thread through the supported app-server API.

Output is a private review candidate. Read it and apply logs/README.md before Git.
"""
import argparse
import json
import subprocess
import sys
from pathlib import Path
from uuid import UUID


def request(proc, number, method, params):
    proc.stdin.write(json.dumps(dict(id=number, method=method, params=params)) + "\n")
    proc.stdin.flush()
    for line in proc.stdout:
        message = json.loads(line)
        if message.get("id") == number:
            if "error" in message:
                raise RuntimeError(message["error"].get("message", "History unavailable"))
            return message["result"]
    raise RuntimeError("History reader stopped before the reply")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("thread_id", type=UUID)
    args = parser.parse_args()
    proc = subprocess.Popen(
        ["codex", "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL, text=True,
    )
    try:
        request(proc, 1, "initialize", {
            "clientInfo": {"name": "robotutor_history_reader", "version": "1"},
            "capabilities": {"experimentalApi": True},
        })
        thread = request(proc, 2, "thread/read", {
            "threadId": str(args.thread_id), "includeTurns": True
        })["thread"]
        root = Path(__file__).resolve().parents[1]
        if Path(thread["cwd"]).resolve() != root:
            raise ValueError("Run the reader from the source thread’s Robotutor checkout.")
        for turn in thread["turns"]:
            for item in turn.get("items", []):
                kind = item["type"]
                if kind == "userMessage":
                    body = "\n".join(
                        c.get("text", "") if c["type"] == "text"
                        else "[OMITTED: non-text user content]"
                        for c in item["content"]
                    )
                    role = "User"
                elif kind in {"agentMessage", "plan"}:
                    role = "Assistant (plan)" if kind == "plan" else "Assistant"
                    body = item["text"]
                else:
                    continue
                print(f"<!-- source-message {turn['id']} {item['id']} -->\n**{role}:** {body}\n")
        print("Review visible messages before logging. Reconcile any planning controls "
              "shown separately by the source interface.", file=sys.stderr)
    finally:
        proc.terminate()
        proc.wait()


if __name__ == "__main__":
    main()
