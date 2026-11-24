"""Persistent, isolated workspaces for fictional test learners."""
from local_platform import lock_file, private_directory
import os
import subprocess
import uuid

PRESETS = {
    "new": ("Brand new", "Starts at Luna’s first getting-to-know-you question."),
    "returning": ("Returning", "One earlier visit about repairing bicycles. Resume the learner reflection."),
    "known": ("Well known", "Two earlier visits, an independently solved rate, and a corrected preference. Open Plan to inspect the evidence."),
}


def workspace(root, preset, tutor, reset=False):
    if preset not in PRESETS:
        raise ValueError("Unknown test learner")
    home = root / preset
    private_directory(home)
    pointer = home / "active"
    with (home / ".lock").open("a") as lock:
        lock_file(lock)
        if pointer.exists() and not reset:
            generation = str(uuid.UUID(pointer.read_text().strip()))
            folder = home / generation
            if not (folder / "personal").is_dir():
                raise ValueError("Test workspace is missing. Use Reset test learner to start again.")
            return folder
        generation = str(uuid.uuid4())
        folder = home / generation
        folder.mkdir(mode=0o700)
        subprocess.run([tutor, "--seed-test-learner", preset, folder / "personal"],
                       check=True, capture_output=True, timeout=30)
        (folder / "assigned").mkdir(mode=0o700)
        temp = home / (".active-" + generation)
        with temp.open("x") as stream:
            stream.write(generation + "\n")
            stream.flush()
            os.fsync(stream.fileno())
        temp.replace(pointer)
        if os.name != "nt":
            directory = os.open(home, os.O_DIRECTORY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
        return folder
