#!/bin/sh
set -eu
umask 077
ROBOTUTOR_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
export ROBOTUTOR_ROOT
: "${CARGO_TARGET_DIR:=$ROBOTUTOR_ROOT/lab/target}"
export CARGO_TARGET_DIR
cargo build --quiet --locked --manifest-path "$ROBOTUTOR_ROOT/desktop/native/Cargo.toml"
cargo build --quiet --locked --manifest-path "$ROBOTUTOR_ROOT/lab/Cargo.toml" --bin tutor
cargo build --quiet --locked --manifest-path "$ROBOTUTOR_ROOT/lms/Cargo.toml"
cargo build --quiet --locked --manifest-path "$ROBOTUTOR_ROOT/desktop/course-window/Cargo.toml"
exec "$CARGO_TARGET_DIR/debug/robotutor-desktop" "$@"
