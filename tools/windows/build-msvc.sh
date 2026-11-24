#!/bin/sh
set -eu
target=x86_64-pc-windows-msvc
export RUSTUP_TOOLCHAIN=1.100.0
rustup toolchain install "$RUSTUP_TOOLCHAIN" --profile minimal --target "$target" --component llvm-tools-preview --no-self-update
while [ "$#" -ge 2 ]; do
    package=$1
    manifest=$2
    shift 2
    cargo xwin build --locked --target "$target" --manifest-path "$manifest" --bin "$package"
    cp "/build/$target/debug/$package.exe" "/output/$package.exe"
done
if [ "${ROBOTUTOR_TEST_RESEARCH:-0}" = 1 ]; then
    cargo xwin test --locked --target "$target" --manifest-path desktop/course-window/Cargo.toml --bin robotutor-research
fi
