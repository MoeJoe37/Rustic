import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
FILES = ["README.md", "docs/progress/engine.svg", "docs/progress/data.json", "docs/progress/summary.md"]


class PublishTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.remote = self.root / "remote.git"
        self.work = self.root / "work"
        self.work.mkdir()
        self.git("init", "--bare", str(self.remote))
        self.git("init", "-b", "main", cwd=self.work)
        self.git("config", "user.name", "Test", cwd=self.work)
        self.git("config", "user.email", "test@example.invalid", cwd=self.work)
        for name in FILES:
            path = self.work / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("baseline\n")
        self.git("add", ".", cwd=self.work)
        self.git("commit", "-m", "source commit", cwd=self.work)
        self.git("remote", "add", "origin", str(self.remote), cwd=self.work)
        self.git("push", "origin", "main", cwd=self.work)
        sha = self.git("rev-parse", "HEAD", cwd=self.work).strip()
        binaries = self.root / "bin"
        binaries.mkdir()
        gh = binaries / "gh"
        gh.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$GH_TEST_LOG"\n')
        gh.chmod(0o755)
        self.log = self.root / "gh.log"
        self.environment = {**os.environ, "PATH": str(binaries) + os.pathsep + os.environ["PATH"],
                            "GH_TOKEN": "local-test-token", "GH_TEST_LOG": str(self.log),
                            "GITHUB_SHA": sha, "GITHUB_REPOSITORY": "test/rustic", "PROGRESS_BRANCH": "main"}

    def git(self, *args, cwd=None):
        return subprocess.run(["git", *args], cwd=cwd or self.root, check=True, capture_output=True, text=True).stdout

    def publish(self):
        return subprocess.run(["bash", str(ROOT / "tools/publish_progress.sh")], cwd=self.work,
                              env=self.environment, check=True, capture_output=True, text=True)

    def test_changed_outputs_are_pushed_and_ci_dispatched_without_unrelated_files(self):
        for name in FILES:
            (self.work / name).write_text("updated\n")
        (self.work / "unrelated.rs").write_text("must not be pushed")
        self.publish()
        self.assertEqual(self.git("--git-dir", str(self.remote), "show", "main:README.md"), "updated\n")
        changed = self.git("diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD", cwd=self.work).splitlines()
        self.assertEqual(set(changed), set(FILES))
        self.assertIn("workflow run ci.yml --repo test/rustic --ref main", self.log.read_text())

    def test_unchanged_outputs_do_not_commit_or_dispatch_again(self):
        self.publish()
        self.assertEqual(self.git("rev-parse", "HEAD", cwd=self.work).strip(), self.environment["GITHUB_SHA"])
        self.assertNotIn("workflow run", self.log.read_text())

    def test_newer_source_commit_is_never_overwritten(self):
        (self.work / "new-source.rs").write_text("newer source")
        self.git("add", "new-source.rs", cwd=self.work)
        self.git("commit", "-m", "newer source", cwd=self.work)
        self.git("push", "origin", "main", cwd=self.work)
        newer = self.git("rev-parse", "HEAD", cwd=self.work).strip()
        self.git("reset", "--hard", self.environment["GITHUB_SHA"], cwd=self.work)
        (self.work / "README.md").write_text("stale generated output\n")
        self.publish()
        self.assertEqual(self.git("--git-dir", str(self.remote), "rev-parse", "refs/heads/main").strip(), newer)
        self.assertNotIn("workflow run", self.log.read_text())


if __name__ == "__main__":
    unittest.main()
