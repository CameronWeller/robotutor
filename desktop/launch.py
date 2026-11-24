"""Compatibility entry point for scripts that used the earlier Python launcher."""
from pathlib import Path
import os
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
launcher = root / ("Robotutor.cmd" if os.name == "nt" else "Robotutor.command")
if __name__ == "__main__":
    sys.exit(subprocess.call([str(launcher), *sys.argv[1:]], cwd=root))
