#!/usr/bin/env python3
"""Compatibility entry point for the native Rust terminal tutor."""
import os
from pathlib import Path
import shutil
import sys


def main():
    root = Path(__file__).resolve().parent
    cargo = shutil.which("cargo")
    if cargo:
        os.execv(cargo, [cargo, "run", "--quiet", "--locked", "--manifest-path",
                         str(root / "Cargo.toml"), "--bin", "tutor", "--", *sys.argv[1:]])
    target = Path(os.environ.get("CARGO_TARGET_DIR", root / "target"))
    for profile in ("debug", "release"):
        binary = target / profile / "tutor"
        if binary.is_file():
            os.execv(binary, [str(binary), *sys.argv[1:]])
    sys.exit("Build the Rust tutor with cargo build --locked --manifest-path lab/Cargo.toml --bin tutor.")


if __name__ == "__main__":
    main()
