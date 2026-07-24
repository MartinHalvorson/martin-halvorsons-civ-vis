from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))

import civvis_push_guard as guard


ZERO = "0" * 40
OLD = "1" * 40
NEW = "2" * 40
BRANCH = "agent/render-win-02/codex-47/task-20260724T005500Z-a31f"


class PushGuardTests(unittest.TestCase):
    def validate(self, remote_ref, *, local_sha=NEW, remote_sha=ZERO, ancestor=True):
        calls = []

        def is_ancestor(older, newer):
            calls.append((older, newer))
            return ancestor

        error = guard.validate_push_update(
            "refs/heads/local",
            local_sha,
            remote_ref,
            remote_sha,
            is_ancestor,
        )
        return error, calls

    def test_every_main_update_is_rejected(self):
        error, _ = self.validate("refs/heads/main")
        self.assertIn("main", error)
        error, _ = self.validate("refs/heads/main", local_sha=ZERO, remote_sha=OLD)
        self.assertIn("main", error)

    def test_valid_new_task_branch_is_allowed(self):
        error, calls = self.validate(f"refs/heads/{BRANCH}")
        self.assertIsNone(error)
        self.assertEqual(calls, [])

    def test_malformed_new_branch_is_rejected(self):
        error, _ = self.validate("refs/heads/feature/shared")
        self.assertIn("development branch must match", error)

    def test_non_main_branch_deletion_is_allowed_for_cleanup(self):
        error, _ = self.validate(
            "refs/heads/legacy/shared", local_sha=ZERO, remote_sha=OLD
        )
        self.assertIsNone(error)

    def test_fast_forward_is_allowed_and_rewrite_is_rejected(self):
        remote_ref = f"refs/heads/{BRANCH}"
        error, calls = self.validate(remote_ref, remote_sha=OLD)
        self.assertIsNone(error)
        self.assertEqual(calls, [(OLD, NEW)])
        error, _ = self.validate(remote_ref, remote_sha=OLD, ancestor=False)
        self.assertIn("non-fast-forward", error)

    def test_non_branch_refs_are_untouched(self):
        error, calls = self.validate("refs/tags/v1", remote_sha=OLD)
        self.assertIsNone(error)
        self.assertEqual(calls, [])


if __name__ == "__main__":
    unittest.main()
