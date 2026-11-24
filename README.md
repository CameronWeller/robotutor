# Robotutor

Robotutor is a Rust tutor for one learner across visits. Start the desktop with Robotutor.command on macOS or Robotutor.cmd on Windows. Run the Linux lab suite with `cargo test --locked --manifest-path lab/Cargo.toml --all-targets`.

The LMS and course interface are included in this tree. The private development archive is optional for contributors. Authorized maintainers can run `git submodule update --init logs` and submit reviewed Markdown using `python3 tools/append_transcript.py --entry ID logs/YYYY/MM/DD/codex-topic-01.md < reviewed.md`. The helper pushes directly to the archive. Refresh its local branch before intentionally checkpointing the gitlink.

Robotutor is licensed under AGPL-3.0. Nunito is distributed under the accompanying SIL Open Font License in web/brand/OFL.txt.
