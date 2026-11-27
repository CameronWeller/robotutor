# Working on Robotutor

Run the locked lab suite and the tests for each changed client. For learner-facing changes, exercise the interaction using fictional learners and inspect the view before and after at the affected sizes.

Authorized maintainers record visible development exchanges in the private logs submodule after review. Keep credentials and private learner information in the owner’s workspace. Use stable entry IDs and retry the appender with --retry-pending to recover interrupted publishes. Review teaching commitments and source citations inside the archive. Use fictional examples in public tests and issues.

## Transcript preservation

The owner enables automatic Claude Code and Codex recording for a Robotutor checkout with `sh tools/claude-transcripts install`. Both project hooks register native session files and append dialogue plus recoverable JSONL at commit and turn end. Git commits pin the resulting private archive revision. The JSONL preserves eligible native records and marks omitted events. It includes tool identity and status. Hidden reasoning, internal instructions, opaque tool payloads and binary attachments are excluded. Known credential formats are redacted. Verify that GitHub CLI is signed in as the archive owner and initialize the private logs submodule with `git submodule update --init logs`. Restore JSONL with `sh tools/claude-transcripts restore-raw PARTS... --output DIR`.

Archived records and commits are permanent. Append to the archive only through `tools/append_transcript.py` or `tools/claude-transcripts`, which never rewrite an entry. Do not edit or delete files under `logs/`, move the gitlink backwards, commit with `--no-verify`, force-push, or rewrite pushed history with rebase, `filter-branch` or `filter-repo`. The Claude hooks refuse these commands, and the pre-push hook refuses pushes that replace commits. Keep secrets out of the conversation in the first place, since redaction only recognizes common formats. If one is pasted, rotate it and report it to the owner. Do not try to remove it from the archive.
