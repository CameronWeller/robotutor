"""Desktop storage and subprocess operations that differ across the supported operating systems."""
import os
from pathlib import Path
import subprocess
import sys


def state_home():
    if os.getenv("XDG_STATE_HOME"):
        return Path(os.environ["XDG_STATE_HOME"])
    if os.name == "nt":
        return Path(os.environ["LOCALAPPDATA"])
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support"
    return Path.home() / ".local/state"


def private_directory(path):
    if os.name == "nt":
        subprocess.run([os.environ["ROBOTUTOR_BIN"], "--init-state-dir", path],
                       check=True, creationflags=subprocess.CREATE_NO_WINDOW)
    else:
        path.mkdir(mode=0o700, parents=True, exist_ok=True)


def lock_file(stream):
    if os.name == "nt":
        import errno
        import msvcrt
        stream.seek(0)
        while True:
            try:
                msvcrt.locking(stream.fileno(), msvcrt.LK_LOCK, 1)
                return
            except OSError as exc:
                if exc.errno not in (errno.EACCES, errno.EDEADLK):
                    raise
    else:
        import fcntl
        fcntl.flock(stream, fcntl.LOCK_EX)


def background_options():
    if os.name == "nt":
        return {"creationflags": subprocess.CREATE_NO_WINDOW | subprocess.CREATE_NEW_PROCESS_GROUP}
    return {"start_new_session": True}


def quiet_process():
    """Keep a console program started by a windowless Windows parent off the desktop."""
    if os.name != "nt":
        return {}
    info = subprocess.STARTUPINFO()
    info.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    info.wShowWindow = subprocess.SW_HIDE
    return {"creationflags": subprocess.CREATE_NO_WINDOW, "startupinfo": info}
