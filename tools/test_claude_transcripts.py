import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import claude_transcripts as ct

SESSION = "00000000-0000-4000-8000-00000000c1a0"
TOOL = Path(__file__).resolve().parent


def line(kind, content, ident, **extra):
    return json.dumps({"type": kind, "uuid": ident, "timestamp": "2026-11-25T21:00:00.000Z",
                       "message": {"role": kind, "content": content}, **extra}) + "\n"


def git(repo, *args):
    return subprocess.run(["git", "-c", "protocol.file.allow=always", "-C", str(repo), *args],
                          capture_output=True, text=True, check=True).stdout.strip()


class RedactionTests(unittest.TestCase):
    def test_known_credentials_are_replaced(self):
        fake = "ghp_" + "A1b2C3d4" * 5
        text = ct.redact(f"token {fake} and https://ada:hunter2hunter2@example.invalid/x "
                         "API_KEY=abc123def456ghi789 Authorization: Bearer abcdefghijklmnop1234")
        self.assertNotIn(fake, text)
        self.assertNotIn("hunter2", text)
        self.assertNotIn("abc123def456ghi789", text)
        self.assertNotIn("abcdefghijklmnop1234", text)
        self.assertIn("https://[REDACTED:url-credentials]@example.invalid/x", text)
        self.assertIn("API_KEY=[REDACTED:assigned-secret]", text)

    def test_ordinary_prose_and_code_survive(self):
        text = "max_tokens: 1000 and token = request.headers['x'] for the learner"
        self.assertEqual(ct.redact(text), text)

    def test_entry_markers_cannot_be_forged(self):
        self.assertNotIn("<!-- transcript-entry ", ct.redact("<!-- transcript-entry x y -->"))


class RawTests(unittest.TestCase):
    def test_long_lines_fences_and_subagents_round_trip(self):
        with tempfile.TemporaryDirectory() as tmp:
            main = Path(tmp) / f"{SESSION}.jsonl"
            agent = Path(tmp) / SESSION / "subagents" / "agent-1.jsonl"
            agent.parent.mkdir(parents=True)
            long = line("user", [{"type": "text", "text": "x" * (ct.ENTRY_LIMIT * 2 + 7)}], "r-0000-0009")
            fenced = line("assistant", [{"type": "text", "text": "````\ncode\n````"}], "a-0000-0009")
            main.write_text(long + fenced + '{"partial":', encoding="utf-8")
            agent.write_text(line("user", "Delegated task.", "u-0000-0009"), encoding="utf-8")
            body = []
            for name, path in ct.raw_sources(main):
                for _, entries in ct.raw_entries(name, path, 0):
                    body += [f"<!-- transcript-entry {entry} {'0' * 64} -->\n{content}" for content, entry in entries]
            record = Path(tmp) / "raw.md"
            record.write_text("\n".join(body), encoding="utf-8")
            out = {p.name: p.read_text(encoding="utf-8") for p in ct.restore_raw([record], Path(tmp) / "out")}
            self.assertEqual(out["main.jsonl"], long + fenced)
            self.assertEqual(out["subagents__agent-1.jsonl"], agent.read_text(encoding="utf-8"))


class GuardTests(unittest.TestCase):
    def test_bypass_and_rewrite_commands_are_refused(self):
        for command in ("git commit --no-verify -m x", "git commit -nm x", "git push --force origin b",
                        "git push -f", "git push origin +main", "cd x && git push --force-with-lease",
                        "git filter-repo --path logs"):
            self.assertTrue(ct.guard(command), command)

    def test_normal_commands_are_allowed(self):
        for command in ("git commit -m 'Add learner check'", "git push -u origin claude/x",
                        "git status", "echo --force"):
            self.assertIsNone(ct.guard(command), command)


class CaptureTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        self.remote = root / "archive.git"
        seed = root / "seed"
        subprocess.run(["git", "init", "-q", "--bare", "-b", "main", str(self.remote)], check=True)
        subprocess.run(["git", "init", "-q", "-b", "main", str(seed)], check=True)
        for repo in (seed,):
            git(repo, "config", "user.name", "Test")
            git(repo, "config", "user.email", "test@example.invalid")
        (seed / ".archive-format").write_text("robotutor-development-transcripts-v1\n")
        (seed / "coverage.md").write_text("# Coverage\n")
        git(seed, "add", ".")
        git(seed, "commit", "-qm", "Start archive")
        git(seed, "push", "-q", str(self.remote), "main")

        self.repo = root / "code"
        subprocess.run(["git", "init", "-q", "-b", "main", str(self.repo)], check=True)
        git(self.repo, "config", "user.name", "Test")
        git(self.repo, "config", "user.email", "test@example.invalid")
        git(self.repo, "submodule", "add", "-q", self.remote.as_uri(), "logs")
        git(self.repo / "logs", "config", "user.name", "Test")
        git(self.repo / "logs", "config", "user.email", "test@example.invalid")
        (self.repo / "tools").mkdir()
        for name in ("claude-transcripts", "claude_transcripts.py", "append_transcript.py", "session_records.py"):
            (self.repo / "tools" / name).write_bytes((TOOL / name).read_bytes())
        (self.repo / "desktop").mkdir()
        (self.repo / "desktop/local_platform.py").write_bytes(
            (TOOL.parent / "desktop/local_platform.py").read_bytes())
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "Start code")
        git(self.repo, "config", "robotutor.archiveSessions", "true")
        self.assertEqual(ct.install_git_hooks(self.repo), [])

        self.transcript = root / "claude" / "projects" / "-code" / f"{SESSION}.jsonl"
        self.transcript.parent.mkdir(parents=True)
        self.env = patch.dict(os.environ, {"CLAUDE_CONFIG_DIR": str(root / "claude"),
                                           "CLAUDE_CODE_SESSION_ID": SESSION,
                                           "GIT_ALLOW_PROTOCOL": "file"})
        self.env.start()
        self.addCleanup(self.env.stop)

    def write(self, *lines):
        with self.transcript.open("a", encoding="utf-8") as out:
            out.write("".join(lines))

    def commit(self, name):
        (self.repo / name).write_text(name)
        git(self.repo, "add", name)
        return subprocess.run(["git", "-c", "protocol.file.allow=always", "-C", str(self.repo),
                               "commit", "-qm", f"Add {name}"], capture_output=True, text=True)

    def archived(self, raw=False):
        git(self.repo / "logs", "fetch", "-q", "origin", "main")
        files = git(self.repo / "logs", "ls-tree", "-r", "--name-only", "FETCH_HEAD").split()
        record = next(f for f in files if f.startswith("2026/11/25/claude-session-")
                      and f.endswith("-raw-01.md" if raw else ".md") and ("-raw-" in f) == raw)
        return git(self.repo / "logs", "show", f"FETCH_HEAD:{record}")

    def test_each_commit_pins_the_dialogue_so_far(self):
        fake = "sk-ant-" + "x" * 30
        self.write(line("user", "Please add a fictional learner check.", "u-0000-0001"),
                   line("assistant", [{"type": "thinking", "thinking": "private reasoning"},
                                      {"type": "text", "text": f"Adding it now with {fake}."},
                                      {"type": "tool_use", "name": "Bash", "input": {
                                          "command": "cat secret.txt", "description": "Read the fixture"}}],
                        "a-0000-0001"),
                   line("user", [{"type": "tool_result", "content": "tool output"}], "r-0000-0001"),
                   line("user", "<system-reminder>context</system-reminder>", "m-0000-0001", isMeta=True),
                   line("assistant", [{"type": "tool_use", "name": "mcp__hearthbot__post_message",
                                       "input": {"text": "Done. The check passes."}}], "a-0000-0002"))
        result = self.commit("one.txt")
        self.assertEqual(result.returncode, 0, result.stderr)
        text = self.archived()
        self.assertIn("Please add a fictional learner check.", text)
        self.assertIn("Tool call: Bash (Read the fixture)", text)
        self.assertIn("Reply to the user:\n\nDone. The check passes.", text)
        for hidden in ("private reasoning", "tool output", "system-reminder", "cat secret.txt", fake):
            self.assertNotIn(hidden, text)
        raw = self.archived(raw=True)
        for kept in ("private reasoning", "tool output", "cat secret.txt", "system-reminder"):
            self.assertNotIn(kept, raw)
        self.assertNotIn(fake, raw)
        self.assertIn("Raw transcript, part 1", text)
        folder = self.repo / "restored"
        record = self.repo / "raw.md"
        record.write_text(raw + "\n", encoding="utf-8")
        restored = ct.restore_raw([record], folder)
        source = self.transcript.read_text(encoding="utf-8").splitlines()
        rebuilt = restored[0].read_text(encoding="utf-8").splitlines()
        self.assertEqual(len(source), len(rebuilt))
        self.assertEqual(source[0], rebuilt[0])  # Untouched lines keep their exact bytes.
        self.assertIn("[REDACTED:anthropic-key]", rebuilt[1])
        record.unlink()
        first = git(self.repo, "ls-tree", "HEAD", "logs").split()[2]
        self.assertEqual(first, git(self.repo / "logs", "rev-parse", "HEAD"))
        self.assertIn("claude-session-", git(self.repo / "logs", "show", "HEAD:coverage.md"))

        # Losing local state must not duplicate or rewrite published entries.
        (self.repo / ".git/claude-transcripts/sessions.json").unlink()
        self.write(line("user", "Now the second change.", "u-0000-0002"))
        result = self.commit("two.txt")
        self.assertEqual(result.returncode, 0, result.stderr)
        text = self.archived()
        self.assertEqual(text.count("Please add a fictional learner check."), 1)
        self.assertEqual(self.archived(raw=True).count("Please add a fictional learner check."), 1)
        self.assertIn("Now the second change.", self.archived(raw=True))
        self.assertIn("Now the second change.", text)
        second = git(self.repo, "ls-tree", "HEAD", "logs").split()[2]
        self.assertNotEqual(first, second)
        git(self.repo / "logs", "merge-base", "--is-ancestor", first, second)

        # A commit with nothing new to say leaves the gitlink alone.
        result = self.commit("three.txt")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(second, git(self.repo, "ls-tree", "HEAD", "logs").split()[2])

    def test_codex_stop_archives_native_dialogue_without_private_model_input(self):
        codex_home = self.repo.parent / "codex-home"
        path = codex_home / "sessions" / f"rollout-{SESSION}.jsonl"
        path.parent.mkdir(parents=True)
        rows = [
            {"type": "event_msg", "payload": {"type": "user_message", "message": "Review my visit."}},
            {"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "injected-context"}]}},
            {"type": "response_item", "payload": {"type": "reasoning", "content": "hidden-reason"}},
            {"type": "turn_context", "payload": {"developer_instructions": "private-instructions"}},
            {"type": "event_msg", "payload": {"type": "agent_message", "message": "Reviewed."}},
        ]
        path.write_text("".join(json.dumps(dict(row, timestamp="2026-11-25T21:00:00Z")) + "\n" for row in rows))
        event = {"session_id": SESSION, "transcript_path": str(path), "hook_event_name": "Stop"}
        env = dict(os.environ, CODEX_HOME=str(codex_home))
        env.pop("CLAUDE_CODE_SESSION_ID", None)
        for _ in range(2):
            subprocess.run(["sh", "tools/claude-transcripts", "codex-hook"], cwd=self.repo,
                           input=json.dumps(event), env=env, text=True, check=True)
        name = f"2026/11/25/codex-session-{ct.tag(SESSION, 'codex')}"
        raw = git(self.repo / "logs", "show", f"HEAD:{name}-raw-01.md")
        dialogue = git(self.repo / "logs", "show", f"HEAD:{name}.md")
        for body in (raw, dialogue):
            self.assertEqual(body.count("Review my visit."), 1)
            self.assertIn("Reviewed.", body)
            for hidden in ("injected-context", "private-instructions", "hidden-reason"):
                self.assertNotIn(hidden, body)
        self.assertEqual(git(self.repo, "ls-files", "-s", "--", "logs").split()[1],
                         git(self.repo / "logs", "rev-parse", "HEAD"))

    def test_path_commit_and_agent_hook_include_the_checkpoint(self):
        self.write(line("user", "Commit only this file.", "u-0000-0004"))
        event = {"hook_event_name": "PreToolUse", "session_id": SESSION, "tool_name": "Bash",
                 "transcript_path": str(self.transcript), "tool_input": {"command": "git commit -m x path.txt"}}
        result = subprocess.run(["sh", "tools/claude-transcripts", "hook"], cwd=self.repo, input=json.dumps(event),
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        (self.repo / "path.txt").write_text("x")
        git(self.repo, "add", "path.txt")
        git(self.repo, "reset", "-q", "--", "logs")  # Only the path is staged by the person.
        result = subprocess.run(["git", "-C", str(self.repo), "commit", "-qm", "Path", "path.txt"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(git(self.repo, "ls-tree", "HEAD", "logs").split()[2],
                         git(self.repo / "logs", "rev-parse", "HEAD"))
        self.assertIn("Commit only this file.", self.archived())

    def test_agent_cannot_edit_the_archive(self):
        event = {"hook_event_name": "PreToolUse", "session_id": SESSION, "tool_name": "Write",
                 "tool_input": {"file_path": str(self.repo / "logs/coverage.md")}}
        result = subprocess.run(["sh", "tools/claude-transcripts", "hook"], cwd=self.repo, input=json.dumps(event),
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)

    def test_commit_stops_when_the_archive_is_unreachable(self):
        self.write(line("user", "Unarchived words.", "u-0000-0003"))
        git(self.repo / "logs", "remote", "set-url", "origin", str(self.remote) + "-missing")
        result = self.commit("blocked.txt")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Transcript capture failed", result.stderr)
        self.assertEqual(git(self.repo, "log", "--format=%s", "-1"), "Start code")

    def test_pre_push_refuses_to_replace_commits(self):
        old = git(self.repo, "rev-parse", "HEAD")
        (self.repo / "x").write_text("x")
        git(self.repo, "add", "x")
        git(self.repo, "commit", "-qm", "x", "--no-verify")
        new = git(self.repo, "rev-parse", "HEAD")
        self.assertEqual(ct.pre_push(self.repo, [f"refs/heads/main {new} refs/heads/main {old}"]), [])
        self.assertTrue(ct.pre_push(self.repo, [f"refs/heads/main {old} refs/heads/main {new}"]))


if __name__ == "__main__":
    unittest.main()
