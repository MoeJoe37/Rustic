from pathlib import Path
import sys
import json
import hashlib
import tempfile
import unittest


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import check_repository  # noqa: E402


class DebtGuardrailTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.saved_root = check_repository.ROOT
        check_repository.ROOT = self.root
        check_repository.ERRORS.clear()
        self.addCleanup(self.restore)
        (self.root / "docs").mkdir()
        (self.root / "docs/TECH_DEBT.md").write_text("### TD-01 — Known\n", encoding="utf-8")

    def restore(self):
        check_repository.ROOT = self.saved_root
        check_repository.ERRORS.clear()

    def write(self, relative, text):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def test_debt_markers_must_cite_a_recorded_entry(self):
        files = [
            self.write("src/ok.rs", "// TODO(TD-01): known\n"),
            self.write("src/bare.rs", "// TODO: unrecorded\n"),
            self.write("src/unknown.py", "# FIXME TD-99 missing\n"),
            self.write("tests/fixtures/data.rs", "// XXX upstream fixture text\n"),
        ]
        check_repository.check_debt_markers(files)
        self.assertEqual(len(check_repository.ERRORS), 2, check_repository.ERRORS)
        self.assertIn("bare.rs", check_repository.ERRORS[0])
        self.assertIn("TD-99", check_repository.ERRORS[1])

    def test_external_dependencies_are_rejected(self):
        self.write("Cargo.toml", '[workspace]\nmembers = ["a"]\n')
        self.write("a/Cargo.toml", '[dependencies]\nlocal = { path = "../b" }\nserde = "1"\n'
                   '[target.\'cfg(unix)\'.dev-dependencies]\nlibc = { version = "0.2" }\n')
        self.write("Cargo.lock", 'version = 4\n[[package]]\nname = "a"\nversion = "0.1.0"\n'
                   '[[package]]\nname = "serde"\nversion = "1.0.0"\n'
                   'source = "registry+https://github.com/rust-lang/crates.io-index"\n')
        check_repository.check_dependencies(["a"])
        joined = "\n".join(check_repository.ERRORS)
        self.assertIn("'serde'", joined)
        self.assertIn("'libc'", joined)
        self.assertIn("lockfile contains external package 'serde'", joined)
        self.assertNotIn("'local'", joined)

    def test_reviewed_dependency_features_checksums_and_notices_are_exact(self):
        self.write("Cargo.toml", '[workspace]\nmembers = ["a"]\n')
        self.write("a/Cargo.toml", '[dependencies]\nfont = { version = "=1.0.0", default-features = false, features = ["std"] }\n')
        lock = 'version = 4\n[[package]]\nname = "font"\nversion = "1.0.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "abc"\n'
        self.write("Cargo.lock", lock)
        self.write("docs/adr/review.md", "Reviewed font adapter\n")
        notice = self.write("third_party/licenses/font.txt", "MIT notice\n")
        review = {"decision": "docs/adr/review.md", "direct": {"a:font": {"version": "=1.0.0", "default-features": False, "features": ["std"]}}, "packages": [{"name": "font", "version": "1.0.0", "source": "registry+https://github.com/rust-lang/crates.io-index", "checksum": "abc", "notices": {"third_party/licenses/font.txt": hashlib.sha256(notice.read_bytes()).hexdigest()}}]}
        self.write("third_party/rust-dependencies.json", json.dumps(review))
        check_repository.check_dependencies(["a"])
        self.assertEqual(check_repository.ERRORS, [])
        self.write("a/Cargo.toml", '[dependencies]\nfont = { version = "=1.0.0", features = ["unsafe"] }\n')
        self.write("Cargo.lock", lock.replace('checksum = "abc"', 'checksum = "changed"'))
        notice.write_text("changed notice\n")
        check_repository.check_dependencies(["a"])
        joined = "\n".join(check_repository.ERRORS)
        self.assertIn("license notice digest changed", joined)
        self.assertIn("exact reviewed entry", joined)
        self.assertIn("without exact reviewed source/checksum", joined)

    def test_every_workflow_uses_the_pinned_toolchain(self):
        self.write(".github/workflows/ci.yml", 'env:\n  RUST_VERSION: "1.99.1"\n')
        self.write(".github/workflows/fuzz.yml", 'env:\n  RUST_VERSION: "1.98.0"\n'
                   "steps:\n  - uses: actions/checkout@v4\n")
        check_repository.check_workflows("1.99.1")
        joined = "\n".join(check_repository.ERRORS)
        self.assertIn("fuzz.yml: RUST_VERSION 1.98.0", joined)
        self.assertIn("not pinned to a full commit SHA: actions/checkout@v4", joined)
        self.assertNotIn("ci.yml", joined)


if __name__ == "__main__":
    unittest.main()
