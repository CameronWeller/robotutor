"""Run the same Rust receipt proof locally or in an explicitly selected Linux verifier."""
import io
from pathlib import Path
import re
import subprocess
import tarfile


RECORDS = {"design", "case", "condition", "decision", "review", "review-grounded", "reassessment",
           "parent-correction"} | {f"{arm}-{stage}" for arm in ("A", "B")
                                   for stage in ("opening", "feedback", "booking", "return", "return-memory", "learner",
                                                 "resume", "resume-memory")}


def invoke_auditor(directory, executable=None, container=None):
    directory = Path(directory).resolve()
    if not container:
        return subprocess.run([str(executable), str(directory)], capture_output=True, timeout=90)
    if not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.-]*", container):
        raise ValueError("invalid verifier container name")
    evidence = io.BytesIO()
    with tarfile.open(fileobj=evidence, mode="w") as archive:
        for path in sorted(directory.glob("*.json")):
            if path.stem not in RECORDS or path.is_symlink() or not path.is_file():
                raise ValueError("unexpected evidence entry")
            data = path.read_bytes()
            if len(data) > 2_000_000:
                raise ValueError("oversized evidence entry")
            entry = tarfile.TarInfo(path.name)
            entry.size = len(data)
            archive.addfile(entry, io.BytesIO(data))
    # Each proof sees its own snapshot, including when the campaign reporter runs concurrently.
    script = ('proof=$(mktemp -d) || exit 1; trap \'rm -rf -- "$proof"\' EXIT; '
              'tar -xf - -C "$proof" && /build/target/debug/examples/teaching_trace "$proof"')
    return subprocess.run(["podman", "exec", "-i", container, "sh", "-c", script],
                          input=evidence.getvalue(), capture_output=True, timeout=90)
