import concurrent.futures
import hashlib
from contextlib import closing
from pathlib import Path
import subprocess
import tempfile
import unittest
from append_transcript import append

class TranscriptTest(unittest.TestCase):
    def test_concurrent_appends_preserve_remote_and_real_index(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo=Path(tmp)/"work"
            remote=Path(tmp)/"remote"
            def git(*args):
                return subprocess.check_output(["git","-C",str(repo),*args],text=True)
            subprocess.run(["git","init","-q","--bare",remote],check=True)
            subprocess.run(["git","init","-q",repo],check=True)
            git("config","user.name","Test")
            git("config","user.email","test@example.invalid")
            original = "**User:** First line.\nSecond line."
            digest = hashlib.sha256(original.encode()).hexdigest()
            historical = (f"<!-- transcript-entry first {digest} -->\n{original}\n"
                          .replace("\n", "\r\n").encode())
            transcript = "2026/11/20/test-concurrent-01.md"
            (repo/transcript).parent.mkdir(parents=True)
            (repo/transcript).write_bytes(historical)
            # Keep the fixture bytes independent of the developer's checkout settings.
            git("config", "core.autocrlf", "false")
            git("add", transcript)
            (repo/".archive-format").write_text("robotutor-development-transcripts-v1")
            git("add",".archive-format")
            git("commit","--allow-empty","-qm","base")
            git("remote","add","origin",str(remote))
            git("push","origin","HEAD:refs/heads/main")
            (repo/"unrelated").write_text("unrelated")
            git("add","unrelated")
            path="logs/2026/11/20/test-concurrent-01.md"
            self.assertEqual(append(repo, path, original, "first"), git("rev-parse", "HEAD").strip())
            second = Path(tmp) / "second"
            subprocess.run(["git", "clone", "-q", "-b", "main", remote, second], check=True)
            subprocess.run(["git", "-C", second, "config", "user.name", "Test"], check=True)
            subprocess.run(["git", "-C", second, "config", "user.email", "test@example.invalid"], check=True)
            with concurrent.futures.ThreadPoolExecutor(4) as pool:
                futures = [pool.submit(append, writer, path, f"Message {i}", str(i))
                           for i, writer in enumerate((repo, second, repo, second))]
                for future in futures:
                    future.result()
            head=append(repo,path,"Message 1","1")
            blob = subprocess.check_output(["git", "-C", repo, "show", f"{head}:{transcript}"])
            self.assertTrue(blob.startswith(historical))
            saved = blob.decode("utf-8")
            for i in range(4):
                self.assertEqual(saved.count(f"Message {i}"),1)
            self.assertEqual(git("ls-tree","--name-only",head),".archive-format\n2026\n")
            self.assertEqual(git("diff","--cached","--name-only"),"unrelated\n")
            with self.assertRaises(ValueError):
                append(repo,"lab/src/lib.rs","hi","x")
            with self.assertRaises(ValueError):
                append(repo,path,"different","1")

    def test_offline_append_survives_and_retries_once(self):
        from append_transcript import retry, outbox
        with tempfile.TemporaryDirectory() as tmp:
            repo, remote = Path(tmp)/"work", Path(tmp)/"remote"
            subprocess.run(["git","init","-q",repo],check=True)
            def git(*args):
                return subprocess.check_output(["git","-C",str(repo),*args],text=True)
            git("config","user.name","Test")
            git("config","user.email","test@example.invalid")
            (repo/".archive-format").write_text("robotutor-development-transcripts-v1")
            git("add",".archive-format")
            git("commit","--allow-empty","-qm","base")
            git("remote","add","origin",str(remote))
            path = "logs/2026/11/22/test-offline-01.md"
            with self.assertRaises(subprocess.CalledProcessError):
                append(repo,path,"Reviewed reply","offline-1")
            with closing(outbox(repo)) as db:
                self.assertEqual(db.execute("SELECT content FROM pending").fetchall(),
                                 [("Reviewed reply",)])
            subprocess.run(["git","init","-q","--bare",remote],check=True)
            git("push","origin","HEAD:refs/heads/main")
            append(repo, path, "Later reply", "online-2")
            retry(repo)
            retry(repo)
            saved = git("show","origin/main:"+path[5:])
            self.assertEqual(saved.count("Reviewed reply"), 1)
            self.assertLess(saved.index("Reviewed reply"), saved.index("Later reply"))
            with closing(outbox(repo)) as db:
                self.assertEqual(db.execute("SELECT count(*) FROM pending").fetchone(), (0,))
