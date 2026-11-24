"""Install the Robotutor recorder for the current Windows user and shared Git checkout."""
import argparse
from contextlib import closing
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
from datetime import datetime, timezone

from session_archive import connect, quiet_process

MARKER = "# Robotutor Codex session archive"


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--repo", type=Path, default=Path.cwd())
    p.add_argument("--include-thread", help="Also enroll this specific development thread")
    args = p.parse_args()
    if os.name != "nt":
        p.error("This login installer is for Windows")
    repo = args.repo.resolve()
    def git(*parts):
        return subprocess.check_output(["git", "-C", repo, *parts], text=True, **quiet_process()).strip()
    if git("remote", "get-url", "origin") not in ("https://github.com/CameronWeller/robotutor.git",
            "git@github.com:CameronWeller/robotutor.git", "https://github.com/CameronWeller/robotutor"):
        p.error("Select the Robotutor development checkout")
    common = Path(git("rev-parse", "--path-format=absolute", "--git-common-dir"))
    base = Path(os.environ["LOCALAPPDATA"]) / "Robotutor" / "SessionArchive"
    state = base / "private"
    hook = Path(git("rev-parse", "--path-format=absolute", "--git-path", "hooks")) / "prepare-commit-msg"
    if not hook.resolve().is_relative_to(common.resolve()):
        p.error("The existing hooks directory is shared outside this checkout. Install the trailer in its hook chain.")
    if hook.exists() and MARKER not in hook.read_text(encoding="utf-8"):
        p.error("An existing prepare-commit-msg hook owns this path. Add the documented trailer command to it.")
    source = Path(__file__).resolve().parent
    if not (repo / "logs/.archive-format").is_file():
        p.error("Initialize the private logs submodule first")
    for path in ("tools/session_archive.py", "tools/append_transcript.py", "tools/read_conversation.py",
                 "tools/verbatim_archive.py", "desktop/local_platform.py"):
        target = base / path
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source.parent / path, target)
    program = base / "tools/session_archive.py"
    with closing(connect(state)) as db:
        current = db.execute("SELECT value FROM config WHERE key='git_common'").fetchone()
        if current and Path(current[0]).resolve() != common.resolve():
            p.error("The recorder is already assigned to a different Robotutor clone")
        with db:
            db.execute("INSERT OR REPLACE INTO config VALUES ('git_common',?)", (str(common),))
        beat = db.execute("SELECT value FROM config WHERE key='heartbeat'").fetchone()
        running = beat and beat[0] != "stopped" and (datetime.now(timezone.utc) - datetime.fromisoformat(beat[0])).total_seconds() < 50
    command = [str(program), "--repo", str(repo), "--state", str(state)]
    if args.include_thread:
        subprocess.run([sys.executable, *command, "include-thread", args.include_thread], check=True)
    hook.parent.mkdir(parents=True, exist_ok=True)
    hook_cmd = " ".join(shlex.quote(s.replace("\\", "/")) for s in [sys.executable, *command, "trailer"])
    hook.write_bytes(("#!/bin/sh\n" + MARKER + '\n[ -n "$CODEX_THREAD_ID" ] || exit 0\n'
                      + hook_cmd + ' "$1" "${2:-}" || echo "Robotutor session trailer unavailable" >&2\n').encode())
    pythonw = Path(sys.executable).with_name("pythonw.exe")
    worker = [str(pythonw), *command, "start"]
    env = dict(os.environ, RT_ARCHIVE_PYTHON=str(pythonw), RT_ARCHIVE_ARGS=subprocess.list2cmdline(worker[1:]))
    create_link = """
    $ErrorActionPreference='Stop'
    $folder=[Environment]::GetFolderPath('Startup')
    $link=(New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $folder 'Robotutor Session Archive.lnk'))
    $link.TargetPath=$env:RT_ARCHIVE_PYTHON
    $link.Arguments=$env:RT_ARCHIVE_ARGS
    $link.WindowStyle=7
    $link.Save()
    """
    subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", create_link],
                   env=env, check=True, **quiet_process())
    (state / "stop").unlink(missing_ok=True)
    if not running:
        subprocess.run(worker, check=True, **quiet_process())
    print("Installed the per-user login recorder and repository commit hook.")
    print(subprocess.list2cmdline([sys.executable, *command, "status"]))


if __name__ == "__main__":
    main()
