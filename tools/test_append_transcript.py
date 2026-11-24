import concurrent.futures
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
            (repo/".archive-format").write_text("robotutor-development-transcripts-v1")
            git("add",".archive-format")
            git("commit","--allow-empty","-qm","base")
            git("remote","add","origin",str(remote))
            git("push","origin","HEAD:refs/heads/main")
            (repo/"unrelated").write_text("unrelated")
            git("add","unrelated")
            path="logs/2026/11/20/test-concurrent-01.md"
            with concurrent.futures.ThreadPoolExecutor(4) as pool:
                list(pool.map(lambda i: append(repo,path,f"Message {i}",str(i)),range(4)))
            head=append(repo,path,"Message 1","1")
            saved=git("show",f"{head}:{path[5:]}")
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
