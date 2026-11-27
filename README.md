# Robotutor

Robotutor is a Rust tutor for one learner across visits. Start the desktop with Robotutor.command on macOS or Robotutor.cmd on Windows. Run the Linux lab suite with `cargo test --locked --manifest-path lab/Cargo.toml --all-targets`.

The LMS and course interface are included in this tree. The private development archive is optional for contributors. Authorized maintainers can run `git submodule update --init logs` and submit reviewed Markdown using `python3 tools/append_transcript.py --entry ID logs/YYYY/MM/DD/codex-topic-01.md < reviewed.md`. The helper pushes directly to the archive. Refresh its local branch before intentionally checkpointing the gitlink.

Robotutor is licensed under AGPL-3.0. Nunito is distributed under the accompanying SIL Open Font License in web/brand/OFL.txt.

## Teaching commitments

Begin with a small, useful discovery question. A learner can correct what Robotutor inferred, inspect why the tutor chose its next move, and return to unfinished work. Credit independent reasoning at the point where it appears, record the help given, and revisit the skill in a different task to assess retention. The same lesson state supports terminal and native desktop clients. The [educational charter](docs/foundation/educational-charter.md) describes how these choices serve the learner.

The contributor suite exercises these decisions with fictional learners.

- During discovery, a learner correction withdraws a proposal that depends on the corrected observation. The tutor waits for agreement before delivering the proposed exercise. [Discovery tests](lab/src/bin/tutor/discovery/tests.rs)
- Sufficient reasoning can move the learner to a new exercise while the saved assessment still records partial understanding. A guided solution schedules another opportunity to try independently. [Advancement tests](lab/src/bin/tutor/judgment/tests.rs) · [Independent review tests](lab/src/bin/tutor/study.rs)
- Reopening a native assignment restores its private plan and preserves a draft saved by another writer. The macOS workflow also opens a fictional course page and checks that selecting its assignment returns the expected ID to the lesson. [Draft recovery tests](desktop/native/src/assignment.rs) · [Course handoff check](desktop/check_native_courses.py)

## Maintaining the private archive

After a reviewed append, refresh the submodule with `git -C logs fetch origin main` and `git -C logs merge --ff-only origin/main` from a clean submodule checkout. Run `python3 tools/transcript_changes.py` to compare dated records to the private review inventory. Run `python3 tools/check_handoffs.py` to verify the recorded code and archive commits. The owner can run `sh tools/claude-transcripts install` to enable Claude Code and Codex session checkpoints. Their project hooks archive dialogue and recoverable JSONL in the private repository, then stage the `logs` gitlink at commit time and turn end. In Codex, review the project hooks in `/hooks` so the client can run them. The gitlink only moves forward. See [AGENTS.md](AGENTS.md#transcript-preservation).

The research browser uses Chromium on Windows (WebView2), macOS, and Linux. On the Unix launchers, Node.js 22.12 or newer and npm install the pinned Electron runtime. Linux builds also require GTK and WebKitGTK 4.1 development libraries for the separate course window. Research recording starts paused. Each lesson owns its notebook and browser profile. Optional Google account verification uses a registered Desktop OAuth client supplied through ROBOTUTOR_GOOGLE_CLIENT_FILE.

On Linux systems that restrict unprivileged user namespaces, Chromium needs a system-owned sandbox helper. The installer can use the helper supplied by Google Chrome or Chromium. If neither is installed, an administrator can install the helper from the pinned Electron distribution under `/usr/local/libexec/robotutor/chrome-sandbox` with owner root and mode 4755. Rerun `node desktop/chromium/install.cjs` after configuring it.
