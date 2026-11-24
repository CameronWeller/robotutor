#!/usr/bin/env python3
"""Compare two tutor binaries on identical private journal prefixes and answers.

Each CASE.jsonl has a sibling CASE.txt containing the next learner message.
Only synthetic or separately authorized provider-bound inputs belong here.
The report includes the actual delivered decisions for inspection in the private output.
"""
import argparse
import json
import os
import selectors
import shutil
import sqlite3
import subprocess
import time
from pathlib import Path


def sample(binary, case, run, policy, budget, real_curl, work_seconds):
    run.mkdir(mode=0o700)
    journal = run / "visit.jsonl"
    shutil.copyfile(case, journal)
    journal.chmod(0o600)
    # The wrapper passes stdin straight through. It records usage and timing only.
    meter = run / "meter"
    meter.mkdir(mode=0o700)
    wrapper = meter / "curl"
    wrapper.write_text(f'''#!/usr/bin/python3
import json, subprocess, sys, time, fcntl
from pathlib import Path
root=Path(__file__).parent
with (root/"meter.lock").open("a") as lock:
    fcntl.flock(lock,fcntl.LOCK_EX)
    ledger=root/"usage.jsonl"
    rows=[json.loads(x) for x in ledger.read_text().splitlines()] if ledger.exists() else []
    if sum(x.get("cost") or 0 for x in rows) >= {budget!r}: sys.exit(22)
start=time.monotonic()
result=subprocess.run([{real_curl!r},*sys.argv[1:]],stdout=subprocess.PIPE)
try: usage=json.loads(result.stdout.rsplit(b"\\n",1)[0]).get("usage",{{}})
except (ValueError,TypeError): usage={{}}
row={{k:usage.get(k) for k in ("total_tokens","cost")}}
row.update(seconds=time.monotonic()-start,endpoint=sys.argv[-1].split("/")[-1],status=result.returncode)
with (root/"meter.lock").open("a") as lock:
    fcntl.flock(lock,fcntl.LOCK_EX)
    with ledger.open("a") as f: print(json.dumps(row),file=f)
sys.stdout.buffer.write(result.stdout)
sys.exit(result.returncode)
''')
    wrapper.chmod(0o700)
    env = dict(os.environ, ROBOTUTOR_DESKTOP="1", ROBOTUTOR_REPLAY_POLICY=policy,
               PATH=str(meter) + os.pathsep + os.environ["PATH"])
    proc = subprocess.Popen([binary, "--workspace", run, "--session", journal.name],
                            env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, start_new_session=True)
    selector = selectors.DefaultSelector()
    selector.register(proc.stdout, selectors.EVENT_READ)
    buffer = bytearray()
    def ready():
        deadline = time.monotonic() + 210
        while time.monotonic() < deadline:
            while b"\n" in buffer:
                line, _, rest = buffer.partition(b"\n")
                buffer[:] = rest
                if line.startswith(b"\x1eREADY "):
                    return json.loads(line.split(b" ", 1)[1])
            if selector.select(max(0, deadline - time.monotonic())):
                chunk = os.read(proc.stdout.fileno(), 65536)
                if not chunk:
                    raise RuntimeError("Tutor stopped before the next prompt")
                buffer.extend(chunk)
        raise TimeoutError("Tutor did not return to the answer surface")
    try:
        opening = ready()
        started = time.monotonic()
        # Identical simulated working interval for both binaries. Stop early only
        # in an explicit zero-wait cold run.
        time.sleep(work_seconds)
        proc.stdin.write((json.dumps(case.with_suffix(".txt").read_text().strip())+"\n").encode())
        proc.stdin.flush()
        submitted = time.monotonic()
        snapshot = ready()
        end = time.monotonic()
        proc.stdin.close()
        proc.wait(timeout=10)
    finally:
        # Disarm the detached idle supervisor in this disposable replica, including
        # when running an older tutor that predates the experiment flags.
        clock = run / "supervision.sqlite3"
        if clock.exists():
            with sqlite3.connect(clock) as db:
                db.execute("UPDATE clock SET due=NULL")
        selector.close()
        if proc.poll() is None:
            proc.terminate()
            proc.wait()
    rows = [json.loads(line) for line in journal.read_text().splitlines()]
    deliveries = [r for r in rows[len(case.read_text().splitlines()):] if r["event"]=="delivered"]
    turn = deliveries[-1] if deliveries else {}
    lesson = turn.get("lesson", {})
    usage_file = meter / "usage.jsonl"
    usage = [json.loads(x) for x in usage_file.read_text().splitlines()] if usage_file.exists() else []
    return dict(case=case.stem, policy=policy, ready_seconds=end-submitted, work_seconds=submitted-started,
                delivered=bool(deliveries) and not snapshot["pending"], exercise=snapshot["exercise"],
                repeated=snapshot["exercise"]==opening["exercise"], text=turn.get("text"),
                selected=lesson.get("prepared_selection"), judgment=(turn.get("judgment") or {}).get("assessment"),
                usage=usage, cost=sum(r["cost"] or 0 for r in usage),
                tokens=sum(r["total_tokens"] or 0 for r in usage))

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--policies", nargs="+", choices=["baseline","history","upstream","downstream","combined"],
                        default=["baseline","combined"])
    parser.add_argument("--work-seconds", type=float, default=15)
    parser.add_argument("--cost-stop-usd", type=float, default=0.25,
                        help="Stop admitting new requests once reported per-trial cost reaches this amount")
    parser.add_argument("cases", nargs="+", type=Path)
    ARGS = parser.parse_args()
    os.umask(0o077)
    ARGS.output.mkdir(mode=0o700)
    real_curl = shutil.which("curl")
    for i, case in enumerate(ARGS.cases):
        for policy in (ARGS.policies if i%2==0 else reversed(ARGS.policies)):
            binary = ARGS.baseline if policy=="baseline" else ARGS.candidate
            row = sample(binary.resolve(),case,ARGS.output / f"{case.stem}-{policy}",policy,ARGS.cost_stop_usd,real_curl,ARGS.work_seconds)
            with (ARGS.output / "results.jsonl").open("a") as stream:
                print(json.dumps(row),file=stream)
            print(case.stem,policy,"delivered",row["delivered"],round(row["ready_seconds"],3),"sec",flush=True)
