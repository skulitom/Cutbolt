"""Exercise commit-boundary failures in disposable repositories; never stage user files."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[1] / "tools/check_repo.py"
SPEC = importlib.util.spec_from_file_location("repo_check", SOURCE)
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class RepositoryBoundary(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="cutbolt-boundary-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.git("init", "--quiet")
        self.git("config", "cutbolt.requireContentPolicy", "true")
        self.write(".git/private-content-policy.json", json.dumps({"patterns": [r"blocked[ _-]?brand"]}))
        self.write(".gitignore", "research-private/\n*.gpr\n")

    def git(self, *args):
        subprocess.run(["git", *args], cwd=self.root, check=True, capture_output=True)

    def write(self, name, value):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value, encoding="utf-8")

    def issues(self, staged=False):
        return CHECK.check_repository(self.root, staged=staged)[0]

    def test_clean_candidate(self):
        self.write("README.md", "Original project documentation.\n")
        self.assertEqual(self.issues(), [])

    def test_untracked_reference(self):
        self.write("README.md", "A BLOCKED-BRAND reference.\n")
        self.assertTrue(any("README.md:1" in x for x in self.issues()))

    def test_staged_bytes_not_clean_working_copy(self):
        self.write("README.md", "blockedbrand\n")
        self.git("add", "README.md")
        self.write("README.md", "Clean working copy.\n")
        self.assertEqual(self.issues(), [])
        self.assertTrue(any("Private content restriction" in x for x in self.issues(staged=True)))

    def test_staged_clean_ignores_unstaged_reference(self):
        self.write("README.md", "Clean indexed bytes.\n")
        self.git("add", "README.md")
        self.write("README.md", "blockedbrand\n")
        self.assertEqual(self.issues(staged=True), [])

    def test_forced_private_text(self):
        self.write("nested/research-private/report.md", "Private findings.\n")
        self.git("add", "-f", "nested/research-private/report.md")
        self.assertTrue(any("Ignored material" in x for x in self.issues(staged=True)))
        self.assertTrue(any("Excluded material" in x for x in self.issues(staged=True)))

    def test_forced_analysis_database(self):
        self.write("analysis.gpr", "Even a text-looking database must stay private.\n")
        self.git("add", "-f", "analysis.gpr")
        self.assertTrue(any("Ignored material" in x for x in self.issues(staged=True)))

    def test_restricted_filename(self):
        self.write("Blocked_Brand.md", "No restricted words in body.\n")
        self.assertTrue(any("restriction in path" in x for x in self.issues()))

    def test_required_policy_missing(self):
        (self.root / ".git/private-content-policy.json").unlink()
        with self.assertRaisesRegex(ValueError, "policy is missing"):
            self.issues(staged=True)

    def test_malformed_policy(self):
        self.write(".git/private-content-policy.json", '{"patterns":[]}')
        with self.assertRaisesRegex(ValueError, "Invalid"):
            self.issues()

    def test_binary_text_extension(self):
        (self.root / "notes.md").write_bytes(b"hidden\0bytes")
        self.git("add", "notes.md")
        self.assertTrue(any("Binary" in x for x in self.issues(staged=True)))

    def test_oversized_indexed_file(self):
        self.write("notes.md", "x" * (CHECK.MAX_BYTES + 1))
        self.git("add", "notes.md")
        self.assertTrue(any("oversized" in x for x in self.issues(staged=True)))

    def test_deleted_index_entry(self):
        self.write("README.md", "blockedbrand\n")
        self.git("add", "README.md")
        self.git("rm", "--cached", "README.md")
        self.assertEqual(self.issues(staged=True), [])


if __name__ == "__main__":
    unittest.main()
