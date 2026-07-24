from pathlib import Path
import concurrent.futures
import os
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

import civvis_collab as collab
import civvis_push_guard as push_guard


def pr(branch, body, *, number=9, draft=True):
    return {
        "number": number,
        "headRefName": branch,
        "body": body,
        "isDraft": draft,
    }


def body(
    machine="render-win-02",
    agent="codex-47",
    paths="`src/game.rs`, `data/**`",
    coordinated="none",
    checked=True,
):
    mark = "x" if checked else " "
    return f"""## Ownership claim

- Machine ID: `{machine}`
- Agent/session ID: `{agent}`
- Task: government cleanup
- Claimed paths: {paths}
- Coordinated with: {coordinated}

## Validation

- [{mark}] Branch started from current `origin/main`
"""


class BranchTests(unittest.TestCase):
    def test_launcher_and_push_guard_branch_formats_stay_in_sync(self):
        self.assertEqual(collab.BRANCH_RE.pattern, push_guard.BRANCH_RE.pattern)

    def test_fleet_branch_is_accepted(self):
        value = "agent/render-win-02/codex-47/government-cleanup-20260723T210500Z-a31f"
        self.assertIsNotNone(collab.BRANCH_RE.fullmatch(value))

    def test_ambiguous_legacy_branch_is_rejected(self):
        self.assertIsNone(collab.BRANCH_RE.fullmatch("agent/government-cleanup"))

    def test_remote_heads_are_parsed_without_symbolic_refs(self):
        raw = (
            "abc123\trefs/heads/main\n"
            "def456\trefs/heads/agent/render-win-02/codex-47/task-20260723T210500Z-a31f\n"
            "999999\trefs/tags/v1\n"
        )
        self.assertEqual(
            collab.parse_remote_heads(raw),
            {
                "main": "abc123",
                "agent/render-win-02/codex-47/task-20260723T210500Z-a31f": "def456",
            },
        )

    def test_shared_push_guard_installs_and_audits_current(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(("git", "init", str(root)), check=True, capture_output=True)
            tools = root / "tools"
            tools.mkdir()
            source = tools / "civvis_push_guard.py"
            source.write_text(
                f"#!/usr/bin/env python3\n# {collab.PUSH_GUARD_MARKER}\n",
                encoding="utf-8",
            )
            target = collab.install_push_guard(root)
            self.assertEqual(target.read_bytes(), source.read_bytes())
            self.assertIsNone(collab.push_guard_error(root))
            if os.name != "nt":
                self.assertTrue(os.access(target, os.X_OK))

    def test_installer_preserves_an_unmanaged_hook(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(("git", "init", str(root)), check=True, capture_output=True)
            tools = root / "tools"
            tools.mkdir()
            (tools / "civvis_push_guard.py").write_text(
                f"# {collab.PUSH_GUARD_MARKER}\n", encoding="utf-8"
            )
            target = collab.common_git_dir(root) / "hooks" / "pre-push"
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            with self.assertRaises(collab.CommandError):
                collab.install_push_guard(root)
            self.assertEqual(target.read_text(encoding="utf-8"), "#!/bin/sh\nexit 0\n")

    def test_simultaneous_installers_leave_one_complete_guard(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(("git", "init", str(root)), check=True, capture_output=True)
            tools = root / "tools"
            tools.mkdir()
            source = tools / "civvis_push_guard.py"
            source.write_text(
                f"#!/usr/bin/env python3\n# {collab.PUSH_GUARD_MARKER}\n",
                encoding="utf-8",
            )
            with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
                targets = list(pool.map(lambda _: collab.install_push_guard(root), range(24)))
            self.assertEqual(len(set(targets)), 1)
            self.assertEqual(targets[0].read_bytes(), source.read_bytes())
            self.assertEqual(list(targets[0].parent.glob(".pre-push.civvis-*")), [])


class ClaimTests(unittest.TestCase):
    def test_claims_are_parsed_from_the_pr_contract(self):
        parsed = collab.parse_claims(body())
        self.assertEqual(parsed["machine"], "render-win-02")
        self.assertEqual(parsed["agent"], "codex-47")
        self.assertEqual(collab.split_paths(parsed["paths"]), ["src/game.rs", "data/**"])

    def test_glob_and_prefix_claims_overlap(self):
        self.assertTrue(collab.claim_patterns_overlap("data/**", "data/units.json"))
        self.assertFalse(collab.claim_patterns_overlap("data/**", "web/index.html"))

    def test_root_wide_and_parent_traversal_claims_are_rejected(self):
        self.assertFalse(collab.valid_claim_pattern("**"))
        self.assertFalse(collab.valid_claim_pattern("../src/game.rs"))


class PolicyTests(unittest.TestCase):
    branch = "agent/render-win-02/codex-47/government-cleanup-20260723T210500Z-a31f"

    def test_valid_draft_claim_passes(self):
        errors = collab.validate_pr(
            pr(self.branch, body()),
            files=["src/game.rs", "data/governments.json"],
            commit_subjects=["claim: government cleanup", "Fix government cleanup"],
        )
        self.assertEqual(errors, [])

    def test_branch_and_body_identity_must_match(self):
        errors = collab.validate_pr(
            pr(self.branch, body(machine="other-host")),
            files=["src/game.rs"],
            commit_subjects=[],
        )
        self.assertIn("Machine ID must match the branch machine component", errors)

    def test_every_changed_file_must_be_claimed(self):
        errors = collab.validate_pr(
            pr(self.branch, body()),
            files=["web/index.html"],
            commit_subjects=[],
        )
        self.assertIn("changed path is not claimed: web/index.html", errors)

    def test_autosync_commits_are_forbidden(self):
        errors = collab.validate_pr(
            pr(self.branch, body()),
            files=["src/game.rs"],
            commit_subjects=["autosync: workstation checkpoint"],
        )
        self.assertTrue(any("autosync commit" in error for error in errors))

    def test_file_overlap_requires_an_explicit_pr_reference(self):
        errors = collab.validate_pr(
            pr(self.branch, body()),
            files=["src/game.rs"],
            commit_subjects=[],
            other_files={5: {"src/game.rs"}},
        )
        self.assertTrue(any("overlap PR #5" in error for error in errors))
        coordinated = collab.validate_pr(
            pr(self.branch, body(coordinated="#5")),
            files=["src/game.rs"],
            commit_subjects=[],
            other_files={5: {"src/game.rs"}},
        )
        self.assertEqual(coordinated, [])

    def test_ready_pr_must_complete_checkboxes(self):
        errors = collab.validate_pr(
            pr(self.branch, body(checked=False), draft=False),
            files=["src/game.rs"],
            commit_subjects=[],
        )
        self.assertIn("ready PRs must complete every validation checkbox", errors)

    def test_main_commit_requires_the_matching_merged_pr_commit(self):
        rows = [
            {"number": 12, "merged_at": "2026-07-23T22:00:00Z", "merge_commit_sha": "abc"},
            {"number": 13, "merged_at": None, "merge_commit_sha": "def"},
        ]
        self.assertEqual(collab.commit_is_pr_backed(rows, "abc"), 12)
        self.assertIsNone(collab.commit_is_pr_backed(rows, "def"))
        self.assertIsNone(collab.commit_is_pr_backed(rows, "missing"))

    def test_only_ahead_or_identical_heads_include_current_main(self):
        self.assertTrue(collab.compare_status_is_current("ahead"))
        self.assertTrue(collab.compare_status_is_current("identical"))
        self.assertFalse(collab.compare_status_is_current("behind"))
        self.assertFalse(collab.compare_status_is_current("diverged"))

    def test_required_checks_must_finish_successfully_before_merge(self):
        merged_at = "2026-07-23T22:37:13Z"
        runs = [
            {
                "name": "cargo-test",
                "started_at": "2026-07-23T22:32:11Z",
                "completed_at": "2026-07-23T22:37:02Z",
                "conclusion": "success",
            },
            {
                "name": "collaboration-policy",
                "started_at": "2026-07-23T22:37:13Z",
                "completed_at": "2026-07-23T22:37:19Z",
                "conclusion": "failure",
            },
        ]
        self.assertEqual(
            collab.required_check_gate_errors(runs, merged_at),
            ["required check collaboration-policy was not green before merge"],
        )

    def test_successful_required_checks_before_merge_pass_the_gate(self):
        runs = [
            {
                "name": name,
                "started_at": "2026-07-23T22:30:00Z",
                "completed_at": "2026-07-23T22:35:00Z",
                "conclusion": "success",
            }
            for name in ("cargo-test", "collaboration-policy")
        ]
        self.assertEqual(
            collab.required_check_gate_errors(runs, "2026-07-23T22:36:00Z"), []
        )


class ShipTests(unittest.TestCase):
    def test_current_pr_names_the_branch_for_repo_scoped_gh(self):
        branch = "agent/m/a/task-20260723T210500Z-a31f"
        with (
            patch.object(collab, "git", return_value=branch),
            patch.object(collab, "gh_json", return_value={"number": 9}) as gh,
        ):
            self.assertEqual(collab.current_pr(Path.cwd()), {"number": 9})
        self.assertEqual(gh.call_args.args[0][2], branch)

    def test_pr_head_waits_for_the_pr_view_to_observe_the_pushed_ref(self):
        branch = "agent/m/a/task-20260723T210500Z-a31f"
        with (
            patch.object(
                collab,
                "current_pr",
                side_effect=[{"headRefOid": "old"}, {"headRefOid": "new"}],
            ),
            patch.object(collab, "remote_heads", return_value={branch: "new"}),
            patch.object(collab.time, "sleep"),
        ):
            result = collab.wait_for_pr_head(
                Path.cwd(),
                branch,
                "new",
                deadline=collab.time.monotonic() + 1,
                poll_seconds=0,
            )
        self.assertEqual(result["headRefOid"], "new")

    def test_ship_requires_a_finished_summary_and_every_checkbox(self):
        draft = {
            "state": "OPEN",
            "headRefName": "agent/m/a/task-20260723T210500Z-a31f",
            "body": """## What changed

Draft claim; implementation is in progress.

## Validation

- [ ] Tests
""",
        }
        errors = collab.ship_pr_errors(draft, draft["headRefName"])
        self.assertTrue(any("checkbox" in error for error in errors))
        self.assertTrue(any("What changed" in error for error in errors))

    def test_ship_accepts_a_documented_validated_feature(self):
        finished = {
            "state": "OPEN",
            "headRefName": "agent/m/a/task-20260723T210500Z-a31f",
            "body": """## What changed

Added the fast shipping path.

## Validation

- [x] Tests
""",
        }
        self.assertEqual(
            collab.ship_pr_errors(finished, finished["headRefName"]), []
        )

    def test_required_checks_use_the_newest_run_for_each_name(self):
        rows = [
            {
                "name": "cargo-test",
                "startedAt": "2026-07-23T22:30:00Z",
                "status": "COMPLETED",
                "conclusion": "FAILURE",
            },
            {
                "name": "cargo-test",
                "startedAt": "2026-07-23T22:35:00Z",
                "status": "COMPLETED",
                "conclusion": "SUCCESS",
            },
            {
                "name": "collaboration-policy",
                "startedAt": "2026-07-23T22:35:01Z",
                "status": "COMPLETED",
                "conclusion": "SUCCESS",
            },
        ]
        self.assertEqual(collab.required_check_state(rows), ("success", []))

    def test_ready_transition_does_not_reuse_an_old_draft_policy_check(self):
        rows = [
            {
                "name": "cargo-test",
                "startedAt": "2026-07-23T22:35:00Z",
                "status": "COMPLETED",
                "conclusion": "SUCCESS",
            },
            {
                "name": "collaboration-policy",
                "startedAt": "2026-07-23T22:34:00Z",
                "status": "COMPLETED",
                "conclusion": "SUCCESS",
            },
        ]
        self.assertEqual(
            collab.required_check_state(
                rows,
                minimum_started={"collaboration-policy": "2026-07-23T22:35:00Z"},
            ),
            ("pending", ["collaboration-policy"]),
        )

    def test_pending_and_failed_checks_are_distinct(self):
        pending = [
            {
                "name": "cargo-test",
                "startedAt": "2026-07-23T22:35:00Z",
                "status": "IN_PROGRESS",
                "conclusion": "",
            }
        ]
        self.assertEqual(
            collab.required_check_state(pending),
            ("pending", ["cargo-test", "collaboration-policy"]),
        )
        failed = pending + [
            {
                "name": "collaboration-policy",
                "startedAt": "2026-07-23T22:35:01Z",
                "status": "COMPLETED",
                "conclusion": "FAILURE",
            }
        ]
        self.assertEqual(
            collab.required_check_state(failed),
            ("failed", ["collaboration-policy"]),
        )

    def test_exact_live_revision_is_accepted_without_git_lookup(self):
        self.assertTrue(
            collab.deployed_commit_covers(
                Path.cwd(), "abc1234", "abc1234567890"
            )
        )


if __name__ == "__main__":
    unittest.main()
