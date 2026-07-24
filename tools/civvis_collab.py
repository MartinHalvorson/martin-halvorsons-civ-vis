#!/usr/bin/env python3
"""Create, validate, and monitor isolated CIVVIS agent tasks.

This tool is intentionally dependency-free so the same workflow runs on macOS,
Linux, and Windows. Git owns local isolation, GitHub draft PRs own fleet-visible
claims, and this script checks the contract at both boundaries.
"""

from __future__ import annotations

import argparse
import datetime as dt
import fnmatch
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import stat
import subprocess
import sys
import time
from typing import Any, Dict, Iterable, List, Optional, Sequence, Set, Tuple
import urllib.error
import urllib.request


REPOSITORY = "MartinHalvorson/CIVVIS"
DEFAULT_BRANCH = "main"
REQUIRED_CHECKS = ("cargo-test", "collaboration-policy")
BRANCH_RE = re.compile(
    r"^agent/(?P<machine>[a-z0-9][a-z0-9-]{0,31})/"
    r"(?P<agent>[a-z0-9][a-z0-9-]{0,31})/"
    r"(?P<task>[a-z0-9][a-z0-9-]{0,47})-"
    r"(?P<stamp>\d{8}T\d{6}Z)-(?P<nonce>[a-f0-9]{4,12})$"
)
ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,31}$")
TASK_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,47}$")
PUSH_GUARD_MARKER = "CIVVIS managed pre-push guard v1"
FIELD_LABELS = {
    "machine": "Machine ID",
    "agent": "Agent/session ID",
    "task": "Task",
    "paths": "Claimed paths",
    "coordinated": "Coordinated with",
}
PLACEHOLDERS = {"", "todo", "tbd", "fill me", "n/a"}


class CommandError(RuntimeError):
    pass


def run(
    args: Sequence[str],
    *,
    cwd: Optional[Path] = None,
    check: bool = True,
    capture: bool = True,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        list(args),
        cwd=str(cwd) if cwd else None,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
        check=False,
    )
    if check and result.returncode:
        rendered = " ".join(args)
        detail = (result.stderr or result.stdout or "").strip()
        raise CommandError(f"{rendered} failed ({result.returncode}): {detail}")
    return result


def git(repo: Path, *args: str, check: bool = True) -> str:
    return run(("git", "-C", str(repo), *args), check=check).stdout.strip()


def repo_root(path: Optional[Path] = None) -> Path:
    start = path or Path.cwd()
    result = run(("git", "-C", str(start), "rev-parse", "--show-toplevel"))
    return Path(result.stdout.strip()).resolve()


def clean_token(value: str) -> str:
    return value.strip().strip("`").strip()


def parse_claims(body: str) -> Dict[str, str]:
    claims: Dict[str, str] = {}
    wanted = {label.lower(): key for key, label in FIELD_LABELS.items()}
    for raw in body.splitlines():
        match = re.match(r"^\s*-\s*([^:]+):\s*(.*?)\s*$", raw)
        if not match:
            continue
        key = wanted.get(match.group(1).strip().lower())
        if key:
            claims[key] = clean_token(match.group(2))
    return claims


def split_paths(raw: str) -> List[str]:
    return [clean_token(item) for item in raw.split(",") if clean_token(item)]


def split_coordination(raw: str) -> Set[int]:
    return {int(value) for value in re.findall(r"#(\d+)", raw)}


def valid_claim_pattern(pattern: str) -> bool:
    if not pattern or pattern in {"*", "**", ".", "./"}:
        return False
    if pattern.startswith(("/", "\\")):
        return False
    parts = pattern.replace("\\", "/").split("/")
    return ".." not in parts and all(parts)


def path_is_claimed(path: str, patterns: Iterable[str]) -> bool:
    normalized = path.replace("\\", "/")
    return any(fnmatch.fnmatchcase(normalized, pattern) for pattern in patterns)


def claim_patterns_overlap(left: str, right: str) -> bool:
    if left == right:
        return True
    left_prefix = left[:-3] if left.endswith("/**") else None
    right_prefix = right[:-3] if right.endswith("/**") else None
    if left_prefix and (right == left_prefix or right.startswith(left_prefix + "/")):
        return True
    if right_prefix and (left == right_prefix or left.startswith(right_prefix + "/")):
        return True
    return fnmatch.fnmatchcase(left, right) or fnmatch.fnmatchcase(right, left)


def claims_overlap(left: Iterable[str], right: Iterable[str]) -> bool:
    return any(claim_patterns_overlap(a, b) for a in left for b in right)


def validate_pr(
    pr: Dict[str, Any],
    *,
    files: Sequence[str],
    commit_subjects: Sequence[str],
    other_files: Optional[Dict[int, Set[str]]] = None,
) -> List[str]:
    number = int(pr.get("number", 0))
    branch = str(pr.get("headRefName") or pr.get("head", {}).get("ref") or "")
    body = str(pr.get("body") or "")
    draft = bool(pr.get("isDraft", pr.get("draft", False)))
    errors: List[str] = []

    branch_match = BRANCH_RE.fullmatch(branch)
    if not branch_match:
        errors.append(
            "head branch must match "
            "agent/<machine>/<agent>/<task>-<YYYYMMDDTHHMMSSZ>-<nonce>"
        )

    claims = parse_claims(body)
    for key, label in FIELD_LABELS.items():
        value = claims.get(key, "").strip().lower()
        if value in PLACEHOLDERS:
            errors.append(f"PR body field '{label}' must be filled")

    if branch_match:
        if claims.get("machine") != branch_match.group("machine"):
            errors.append("Machine ID must match the branch machine component")
        if claims.get("agent") != branch_match.group("agent"):
            errors.append("Agent/session ID must match the branch agent component")

    patterns = split_paths(claims.get("paths", ""))
    invalid = [pattern for pattern in patterns if not valid_claim_pattern(pattern)]
    if invalid:
        errors.append("invalid claimed path patterns: " + ", ".join(invalid))
    for changed in files:
        if patterns and not path_is_claimed(changed, patterns):
            errors.append(f"changed path is not claimed: {changed}")

    for subject in commit_subjects:
        if subject.lower().startswith("autosync:"):
            errors.append(f"mutating autosync commit is forbidden: {subject}")

    coordinated = split_coordination(claims.get("coordinated", ""))
    current_files = set(files)
    for other_number, changed in (other_files or {}).items():
        overlap = sorted(current_files & changed)
        if overlap and other_number not in coordinated:
            preview = ", ".join(overlap[:5])
            errors.append(
                f"changed paths overlap PR #{other_number} without declaring it "
                f"in Coordinated with: {preview}"
            )

    if not draft and re.search(r"^\s*- \[ \]", body, re.MULTILINE):
        errors.append("ready PRs must complete every validation checkbox")

    return errors


def compare_status_is_current(status: str) -> bool:
    """Return whether a PR head contains the current base branch tip."""
    return status in {"ahead", "identical"}


def github_json(path: str, token: str) -> Any:
    url = f"https://api.github.com{path}"
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "User-Agent": "civvis-collaboration-policy",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", "replace")
        raise CommandError(f"GitHub API {path} failed ({exc.code}): {detail}") from exc


def pr_files(repository: str, number: int, token: str) -> List[str]:
    rows = github_json(f"/repos/{repository}/pulls/{number}/files?per_page=100", token)
    return [str(row["filename"]) for row in rows]


def pr_commit_subjects(repository: str, number: int, token: str) -> List[str]:
    rows = github_json(f"/repos/{repository}/pulls/{number}/commits?per_page=100", token)
    return [str(row["commit"]["message"]).splitlines()[0] for row in rows]


def check_pr_action(event_path: Path, token: str, repository: str) -> int:
    event = json.loads(event_path.read_text(encoding="utf-8"))
    if "pull_request" not in event:
        print("collaboration policy: non-PR event, nothing to validate")
        return 0

    current = dict(event["pull_request"])
    current["headRefName"] = current.get("head", {}).get("ref", "")
    current["isDraft"] = current.get("draft", False)
    number = int(current["number"])
    files = pr_files(repository, number, token)
    subjects = pr_commit_subjects(repository, number, token)
    open_prs = github_json(f"/repos/{repository}/pulls?state=open&per_page=100", token)
    other_files = {
        int(other["number"]): set(pr_files(repository, int(other["number"]), token))
        for other in open_prs
        if int(other["number"]) != number
    }
    errors = validate_pr(
        current,
        files=files,
        commit_subjects=subjects,
        other_files=other_files,
    )
    if not current["isDraft"]:
        base_sha = str(current.get("base", {}).get("sha") or "")
        head_sha = str(current.get("head", {}).get("sha") or "")
        if base_sha and head_sha:
            comparison = github_json(
                f"/repos/{repository}/compare/{base_sha}...{head_sha}", token
            )
            if not compare_status_is_current(str(comparison.get("status") or "")):
                errors.append("ready PR branch must include the current main tip")
    if errors:
        for error in errors:
            print(f"::error::{error}")
        print(f"collaboration policy: {len(errors)} violation(s)")
        return 1
    print(
        f"collaboration policy: PR #{number} owns {len(files)} changed path(s) "
        "on a valid single-writer branch"
    )
    return 0


def gh_json(args: Sequence[str], *, cwd: Optional[Path] = None) -> Any:
    result = run(("gh", *args), cwd=cwd)
    return json.loads(result.stdout or "null")


def required_check_state(
    rows: Sequence[Dict[str, Any]],
    *,
    required: Iterable[str] = REQUIRED_CHECKS,
    minimum_started: Optional[Dict[str, str]] = None,
) -> Tuple[str, List[str]]:
    """Summarize the newest check run for every required workflow.

    GitHub can retain several runs with the same name on one PR head after a
    body edit or draft/ready transition. Only the newest eligible run is the
    current gate. ``minimum_started`` lets ``ship`` wait for the policy run
    caused by its own ready-for-review transition instead of accepting an
    older green draft check in the few seconds before the new run appears.
    """
    missing_or_pending: List[str] = []
    failed: List[str] = []
    thresholds = minimum_started or {}
    for name in required:
        candidates = [
            row
            for row in rows
            if str(row.get("name") or row.get("context") or "") == name
            and str(row.get("startedAt") or row.get("started_at") or "")
            >= thresholds.get(name, "")
        ]
        if not candidates:
            missing_or_pending.append(name)
            continue
        latest = max(
            candidates,
            key=lambda row: str(
                row.get("startedAt")
                or row.get("started_at")
                or row.get("completedAt")
                or row.get("completed_at")
                or ""
            ),
        )
        status = str(latest.get("status") or "").lower()
        conclusion = str(
            latest.get("conclusion") or latest.get("state") or ""
        ).lower()
        if status and status != "completed":
            missing_or_pending.append(name)
        elif conclusion not in {"success", "successful"}:
            failed.append(name)
    if failed:
        return "failed", failed
    if missing_or_pending:
        return "pending", missing_or_pending
    return "success", []


def ship_pr_errors(pr: Dict[str, Any], branch: str) -> List[str]:
    """Return reasons a task PR is not yet an honest finished feature."""
    errors: List[str] = []
    if str(pr.get("state") or "").upper() != "OPEN":
        errors.append("the current branch PR is not open")
    if str(pr.get("headRefName") or "") != branch:
        errors.append("the current branch does not own the discovered PR")
    body = str(pr.get("body") or "")
    if re.search(r"^\s*- \[ \]", body, re.MULTILINE):
        errors.append("complete every PR validation checkbox before shipping")
    summary = re.search(
        r"^## What changed\s*(.*?)(?=^## |\Z)",
        body,
        re.MULTILINE | re.DOTALL,
    )
    summary_text = summary.group(1).strip() if summary else ""
    if not summary_text or "implementation is in progress" in summary_text.lower():
        errors.append("replace the draft 'What changed' text with the finished summary")
    return errors


def ref_contains(repo: Path, ancestor: str, descendant: str = "HEAD") -> bool:
    return run(
        ("git", "-C", str(repo), "merge-base", "--is-ancestor", ancestor, descendant),
        check=False,
    ).returncode == 0


def current_pr(repo: Path) -> Dict[str, Any]:
    return dict(
        gh_json(
            (
                "pr",
                "view",
                "--repo",
                REPOSITORY,
                "--json",
                "number,url,state,isDraft,body,headRefName,headRefOid,baseRefOid,"
                "mergeStateStatus,statusCheckRollup,title",
            ),
            cwd=repo,
        )
    )


def merge_current_main(repo: Path) -> bool:
    """Integrate a newly advanced main and independently revalidate the result."""
    fetch_main(repo)
    if ref_contains(repo, "origin/main"):
        return False
    print("main advanced; merging it and rerunning the required local test")
    merged = run(
        ("git", "-C", str(repo), "merge", "--no-edit", "origin/main"),
        check=False,
    )
    if merged.returncode:
        detail = (merged.stderr or merged.stdout or "merge conflict").strip()
        raise CommandError(
            "latest main did not merge cleanly; resolve this task worktree, "
            f"revalidate it, and run ship again: {detail}"
        )
    git(repo, "diff", "--check", "origin/main...")
    run(
        ("cargo", "test", "--release", "--locked"),
        cwd=repo,
        capture=False,
    )
    return True


def local_deploy_root(repo: Path) -> Optional[Path]:
    common = common_git_dir(repo)
    root = common.parent
    return root if (root / "target" / "spectator" / "build.json").is_file() else None


def live_status_commit(url: str, timeout: float = 5.0) -> str:
    try:
        with urllib.request.urlopen(url, timeout=timeout) as response:
            payload = json.load(response)
    except (OSError, ValueError, urllib.error.URLError):
        return ""
    return str(payload.get("commit") or "") if isinstance(payload, dict) else ""


def deployed_commit_covers(repo: Path, deployed: str, merged_sha: str) -> bool:
    if not deployed:
        return False
    if merged_sha.startswith(deployed) or deployed.startswith(merged_sha):
        return True
    resolved = git(repo, "rev-parse", "--verify", deployed, check=False)
    return bool(resolved) and ref_contains(repo, merged_sha, resolved)


def wait_for_local_live_build(
    repo: Path,
    merged_sha: str,
    *,
    url: str,
    timeout_seconds: float,
    poll_seconds: float,
) -> bool:
    if local_deploy_root(repo) is None or timeout_seconds <= 0:
        print("no local production spectator detected; merge is complete")
        return False
    print(f"waiting for the production spectator at {url} to run {merged_sha[:7]}")
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        deployed = live_status_commit(url)
        if deployed_commit_covers(repo, deployed, merged_sha):
            print(f"production spectator is live on {deployed}")
            return True
        time.sleep(max(0.1, poll_seconds))
        fetch_main(repo)
    print(
        "production spectator did not confirm the merged revision before the "
        "live-build timeout"
    )
    return False


def ship_task(args: argparse.Namespace) -> int:
    """Push a finished task, wait for green CI, squash-merge, and verify live."""
    root = repo_root()
    if not shutil.which("gh"):
        raise CommandError("GitHub CLI 'gh' is required to ship a task")
    run(("gh", "auth", "status"), cwd=root)
    branch = git(root, "symbolic-ref", "--quiet", "--short", "HEAD", check=False)
    if not BRANCH_RE.fullmatch(branch):
        raise CommandError("ship must run from this task's conforming agent branch")
    if git(root, "status", "--porcelain"):
        raise CommandError("commit the finished feature and leave the worktree clean first")

    install_push_guard(root)
    fetch_main(root)
    if run(
        ("git", "-C", str(root), "diff", "--quiet", "origin/main...HEAD"),
        check=False,
    ).returncode == 0:
        raise CommandError("the task has no file changes relative to main")

    pr = current_pr(root)
    errors = ship_pr_errors(pr, branch)
    if errors:
        raise CommandError("; ".join(errors))

    deadline = time.monotonic() + max(1.0, args.timeout_seconds)
    ready_thresholds: Dict[str, str] = {}
    while True:
        if time.monotonic() >= deadline:
            raise CommandError("timed out waiting for the task to reach main")

        merged_main = merge_current_main(root)
        if merged_main and git(root, "status", "--porcelain"):
            raise CommandError("main integration left unexpected worktree changes")
        git(root, "diff", "--check", "origin/main...")
        git(root, "push", "origin", f"HEAD:{branch}")
        local_head = git(root, "rev-parse", "HEAD")

        pr = current_pr(root)
        if str(pr.get("headRefOid") or "") != local_head:
            raise CommandError("the PR head changed outside this task's one-writer worktree")
        errors = ship_pr_errors(pr, branch)
        if errors:
            raise CommandError("; ".join(errors))
        if pr.get("isDraft"):
            # Permit a small clock skew while still excluding earlier draft
            # policy runs from the ready-for-review gate.
            threshold = dt.datetime.now(dt.timezone.utc) - dt.timedelta(seconds=5)
            ready_thresholds["collaboration-policy"] = threshold.isoformat().replace(
                "+00:00", "Z"
            )
            run(
                ("gh", "pr", "ready", str(pr["number"]), "--repo", REPOSITORY),
                cwd=root,
            )
            print(f"PR #{pr['number']} is ready; waiting for required checks")

        while True:
            if time.monotonic() >= deadline:
                raise CommandError("timed out waiting for required checks")
            pr = current_pr(root)
            if str(pr.get("state") or "").upper() != "OPEN":
                raise CommandError("the PR closed before ship completed")
            if str(pr.get("headRefOid") or "") != local_head:
                raise CommandError("the PR head changed outside this task's one-writer worktree")

            fetch_main(root)
            if not ref_contains(root, "origin/main"):
                print("main advanced while CI was running; updating this task")
                ready_thresholds.clear()
                break

            state, names = required_check_state(
                pr.get("statusCheckRollup") or [], minimum_started=ready_thresholds
            )
            if state == "failed":
                raise CommandError("required checks failed: " + ", ".join(names))
            if state == "success":
                merge_result = gh_api_write(
                    "PUT",
                    f"repos/{REPOSITORY}/pulls/{pr['number']}/merge",
                    {"merge_method": "squash", "sha": local_head},
                )
                if not merge_result.get("merged"):
                    raise CommandError(
                        "GitHub refused the green squash merge: "
                        + str(merge_result.get("message") or "unknown reason")
                    )
                merged_sha = str(merge_result.get("sha") or "")
                print(f"PR #{pr['number']} squash-merged as {merged_sha[:7]}")
                deletion = run(
                    ("git", "-C", str(root), "push", "origin", "--delete", branch),
                    check=False,
                )
                if deletion.returncode:
                    print("remote branch was already deleted or could not be deleted")
                fetch_main(root)
                wait_for_local_live_build(
                    root,
                    merged_sha,
                    url=args.live_url,
                    timeout_seconds=args.live_timeout_seconds,
                    poll_seconds=args.poll_seconds,
                )
                return 0

            print("waiting on: " + ", ".join(names))
            time.sleep(max(0.1, args.poll_seconds))


def existing_pr_claims(repo: Path) -> List[Dict[str, Any]]:
    rows = gh_json(
        (
            "pr",
            "list",
            "--repo",
            REPOSITORY,
            "--state",
            "open",
            "--limit",
            "100",
            "--json",
            "number,body,headRefName,title",
        ),
        cwd=repo,
    )
    return list(rows)


def parse_remote_heads(raw: str) -> Dict[str, str]:
    heads: Dict[str, str] = {}
    prefix = "refs/heads/"
    for line in raw.splitlines():
        sha, separator, ref = line.partition("\t")
        if separator and ref.startswith(prefix):
            heads[ref[len(prefix) :]] = sha
    return heads


def remote_heads(repo: Path) -> Dict[str, str]:
    return parse_remote_heads(git(repo, "ls-remote", "--heads", "origin"))


def commit_is_pr_backed(rows: Sequence[Dict[str, Any]], sha: str) -> Optional[int]:
    for row in rows:
        if row.get("merged_at") and row.get("merge_commit_sha") == sha:
            return int(row["number"])
    return None


def associated_pr_number(sha: str) -> Optional[int]:
    rows = gh_json(("api", f"repos/{REPOSITORY}/commits/{sha}/pulls"))
    return commit_is_pr_backed(rows or [], sha)


def required_check_gate_errors(
    check_runs: Sequence[Dict[str, Any]],
    merged_at: str,
    required: Iterable[str] = REQUIRED_CHECKS,
) -> List[str]:
    """Report required checks that were not successful before a PR merged."""
    merge_time = dt.datetime.fromisoformat(merged_at.replace("Z", "+00:00"))
    errors: List[str] = []
    for name in required:
        eligible: List[Dict[str, Any]] = []
        for row in check_runs:
            if row.get("name") != name:
                continue
            started_at = str(row.get("started_at") or "")
            if not started_at:
                continue
            started = dt.datetime.fromisoformat(started_at.replace("Z", "+00:00"))
            if started <= merge_time:
                eligible.append(row)
        if not eligible:
            errors.append(f"required check {name} had not started before merge")
            continue
        latest = max(eligible, key=lambda row: str(row.get("started_at") or ""))
        completed_at = str(latest.get("completed_at") or "")
        completed = (
            dt.datetime.fromisoformat(completed_at.replace("Z", "+00:00"))
            if completed_at
            else None
        )
        if latest.get("conclusion") != "success" or not completed or completed > merge_time:
            errors.append(f"required check {name} was not green before merge")
    return errors


def merged_pr_gate_errors(number: int, base_sha: str = "") -> List[str]:
    view = gh_json(
        (
            "pr",
            "view",
            str(number),
            "--repo",
            REPOSITORY,
            "--json",
            "headRefOid,mergedAt",
        )
    )
    head_sha = str(view.get("headRefOid") or "")
    merged_at = str(view.get("mergedAt") or "")
    if not head_sha or not merged_at:
        return ["merged PR metadata is incomplete"]
    errors: List[str] = []
    if base_sha:
        comparison = gh_json(
            (
                "api",
                f"repos/{REPOSITORY}/compare/{base_sha}...{head_sha}",
            )
        )
        if not compare_status_is_current(str(comparison.get("status") or "")):
            errors.append("PR head did not contain current main before merge")
    payload = gh_json(
        (
            "api",
            f"repos/{REPOSITORY}/commits/{head_sha}/check-runs?per_page=100",
        )
    )
    errors.extend(
        required_check_gate_errors(payload.get("check_runs") or [], merged_at)
    )
    return errors


def format_claim_body(
    *,
    machine: str,
    agent: str,
    task: str,
    paths: Sequence[str],
    coordinated: Sequence[int],
) -> str:
    coordination = ", ".join(f"#{number}" for number in coordinated) or "none"
    claimed = ", ".join(f"`{path}`" for path in paths)
    return f"""## Ownership claim

- Machine ID: `{machine}`
- Agent/session ID: `{agent}`
- Task: {task.replace('-', ' ')}
- Claimed paths: {claimed}
- Coordinated with: {coordination}
- Related issue/request: operator request

## What changed

Draft claim; implementation is in progress.

## Validation

- [ ] Branch started from current `origin/main`
- [ ] Ownership/overlap is coordinated above
- [ ] Latest `origin/main` merged before ready
- [ ] `git diff --check origin/main...`
- [ ] `cargo test --release --locked`
- [ ] Relevant focused tests
- [ ] Soak run for engine changes, or reason it is not applicable
- [ ] No unrelated formatting, generated output, or runtime artifacts

## Notes for integration

Squash merge only. Delete the branch after merge.
"""


def validate_identifier(label: str, value: str, pattern: re.Pattern[str]) -> None:
    if not pattern.fullmatch(value):
        raise CommandError(
            f"{label} '{value}' must be lowercase letters, numbers, and hyphens "
            f"and fit the fleet naming limit"
        )


def fetch_main(repo: Path) -> None:
    last_error: Optional[Exception] = None
    for delay in (0, 1, 2):
        if delay:
            time.sleep(delay)
        try:
            git(repo, "fetch", "--prune", "origin", DEFAULT_BRANCH)
            return
        except CommandError as exc:
            last_error = exc
    assert last_error is not None
    raise last_error


def common_git_dir(repo: Path) -> Path:
    raw = Path(git(repo, "rev-parse", "--git-common-dir"))
    return raw.resolve() if raw.is_absolute() else (repo / raw).resolve()


def push_guard_paths(repo: Path) -> Tuple[Path, Path]:
    source = repo / "tools" / "civvis_push_guard.py"
    target = common_git_dir(repo) / "hooks" / "pre-push"
    return source, target


def install_push_guard(repo: Path) -> Path:
    source, target = push_guard_paths(repo)
    if not source.is_file():
        raise CommandError(f"versioned push guard is missing: {source}")
    source_bytes = source.read_bytes()
    if target.is_symlink():
        raise CommandError(f"refusing to replace symlinked pre-push hook: {target}")
    if target.exists():
        existing = target.read_bytes()
        if existing != source_bytes and PUSH_GUARD_MARKER.encode() not in existing:
            raise CommandError(
                f"refusing to overwrite unmanaged pre-push hook: {target}; "
                "preserve and resolve that hook explicitly before retrying"
            )
    target.parent.mkdir(parents=True, exist_ok=True)
    temporary = target.with_name(
        f".{target.name}.civvis-{os.getpid()}-{secrets.token_hex(4)}"
    )
    try:
        with temporary.open("xb") as handle:
            handle.write(source_bytes)
            handle.flush()
            os.fsync(handle.fileno())
        if os.name != "nt":
            temporary.chmod(
                temporary.stat().st_mode
                | stat.S_IXUSR
                | stat.S_IXGRP
                | stat.S_IXOTH
            )
        os.replace(temporary, target)
    finally:
        temporary.unlink(missing_ok=True)
    return target


def push_guard_error(repo: Path) -> Optional[str]:
    source, target = push_guard_paths(repo)
    if not source.is_file():
        return f"versioned push guard is missing: {source}"
    if target.is_symlink():
        return f"local pre-push guard must not be a symlink: {target}"
    if not target.is_file():
        return (
            "local pre-push guard is not installed; run "
            "python3 tools/civvis_collab.py install-hooks"
        )
    if target.read_bytes() != source.read_bytes():
        return (
            "local pre-push guard is outdated or unmanaged; run "
            "python3 tools/civvis_collab.py install-hooks"
        )
    if os.name != "nt" and not os.access(target, os.X_OK):
        return f"local pre-push guard is not executable: {target}"
    return None


def install_hooks_command(args: argparse.Namespace) -> int:
    del args
    target = install_push_guard(repo_root())
    print(f"installed CIVVIS pre-push guard: {target}")
    return 0


def start_task(args: argparse.Namespace) -> int:
    root = repo_root()
    if not shutil.which("gh"):
        raise CommandError("GitHub CLI 'gh' is required to publish the draft claim")
    run(("gh", "auth", "status"), cwd=root)

    configured_machine = git(root, "config", "--get", "civvis.machine", check=False)
    machine = args.machine or configured_machine
    agent = args.agent or os.environ.get("CIVVIS_AGENT_ID", "")
    if not machine:
        raise CommandError("pass --machine once or set: git config civvis.machine <stable-id>")
    if not agent:
        raise CommandError("pass --agent or set CIVVIS_AGENT_ID")
    validate_identifier("machine", machine, ID_RE)
    validate_identifier("agent", agent, ID_RE)
    validate_identifier("task", args.task, TASK_RE)

    paths = [path.replace("\\", "/") for path in args.path]
    if not paths or any(not valid_claim_pattern(path) for path in paths):
        raise CommandError("provide one or more safe repo-relative --path claims")
    coordinated = sorted(set(args.coordinate))

    conflicts: List[Tuple[int, List[str]]] = []
    for pr in existing_pr_claims(root):
        other = split_paths(parse_claims(str(pr.get("body") or "")).get("paths", ""))
        if other and claims_overlap(paths, other):
            conflicts.append((int(pr["number"]), other))
    undeclared = [(number, claim) for number, claim in conflicts if number not in coordinated]
    if undeclared:
        detail = "; ".join(
            f"PR #{number}: {', '.join(claim)}" for number, claim in undeclared
        )
        raise CommandError(
            "claimed paths overlap existing PR ownership; coordinate first and rerun "
            f"with --coordinate <PR>: {detail}"
        )

    stamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    nonce = secrets.token_hex(2)
    branch = f"agent/{machine}/{agent}/{args.task}-{stamp}-{nonce}"
    parent = Path(args.parent).expanduser().resolve() if args.parent else root.parent
    worktree = parent / f"civvis-{args.task}-{nonce}"
    if worktree.exists():
        raise CommandError(f"worktree path already exists: {worktree}")

    if args.dry_run:
        print(json.dumps({"branch": branch, "worktree": str(worktree), "paths": paths}, indent=2))
        return 0

    if args.machine and machine != configured_machine:
        git(root, "config", "civvis.machine", machine)
    for key, value in (
        ("fetch.prune", "true"),
        ("pull.ff", "only"),
        ("push.default", "simple"),
        ("merge.conflictStyle", "zdiff3"),
        ("rerere.enabled", "true"),
        ("rerere.autoupdate", "false"),
    ):
        git(root, "config", key, value)

    install_push_guard(root)

    fetch_main(root)
    git(root, "worktree", "add", "-b", branch, str(worktree), "origin/main")
    git(worktree, "commit", "--allow-empty", "-m", f"claim: {args.task.replace('-', ' ')}")
    git(worktree, "push", "-u", "origin", branch)

    body = format_claim_body(
        machine=machine,
        agent=agent,
        task=args.task,
        paths=paths,
        coordinated=coordinated,
    )
    title = args.title or args.task.replace("-", " ").capitalize()
    result = run(
        (
            "gh",
            "pr",
            "create",
            "--repo",
            REPOSITORY,
            "--draft",
            "--base",
            DEFAULT_BRANCH,
            "--head",
            branch,
            "--title",
            title,
            "--body",
            body,
        ),
        cwd=worktree,
    )
    print(f"worktree: {worktree}")
    print(f"branch:   {branch}")
    print(f"draft PR: {result.stdout.strip()}")
    return 0


def parse_worktrees(raw: str) -> List[Dict[str, str]]:
    rows: List[Dict[str, str]] = []
    current: Dict[str, str] = {}
    for line in raw.splitlines() + [""]:
        if not line:
            if current:
                rows.append(current)
                current = {}
            continue
        key, _, value = line.partition(" ")
        current[key] = value
    return rows


def gh_api_optional(path: str) -> Tuple[int, Any]:
    result = run(("gh", "api", path), check=False)
    if result.returncode:
        return result.returncode, None
    return 0, json.loads(result.stdout)


def gh_api_write(method: str, path: str, payload: Dict[str, Any]) -> Any:
    result = subprocess.run(
        ("gh", "api", "--method", method, path, "--input", "-"),
        input=json.dumps(payload),
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode:
        raise CommandError(
            f"GitHub {method} {path} failed ({result.returncode}): {result.stderr.strip()}"
        )
    return json.loads(result.stdout or "null")


def enforce_github_command(args: argparse.Namespace) -> int:
    root = repo_root()
    permission = gh_json(("api", f"repos/{REPOSITORY}", "--jq", ".permissions.admin"), cwd=root)
    if permission is not True:
        raise CommandError(
            "the active GitHub account is not a repository administrator; "
            "authenticate the MartinHalvorson owner account, then rerun"
        )

    gh_api_write(
        "PATCH",
        f"repos/{REPOSITORY}",
        {
            "allow_squash_merge": True,
            "allow_merge_commit": False,
            "allow_rebase_merge": False,
            "allow_auto_merge": True,
            "delete_branch_on_merge": True,
            "squash_merge_commit_title": "PR_TITLE",
            "squash_merge_commit_message": "PR_BODY",
        },
    )
    gh_api_write(
        "PUT",
        f"repos/{REPOSITORY}/branches/{DEFAULT_BRANCH}/protection",
        {
            "required_status_checks": {
                "strict": True,
                "contexts": ["cargo-test", "collaboration-policy"],
            },
            "enforce_admins": True,
            "required_pull_request_reviews": {
                "dismiss_stale_reviews": False,
                "require_code_owner_reviews": False,
                "required_approving_review_count": 0,
                "require_last_push_approval": False,
                "bypass_pull_request_allowances": {"users": [], "teams": [], "apps": []},
            },
            "restrictions": None,
            "required_conversation_resolution": True,
            "required_linear_history": True,
            "allow_force_pushes": False,
            "allow_deletions": False,
            "block_creations": False,
            "required_signatures": False,
            "lock_branch": False,
        },
    )
    print("GitHub enforcement applied: PR-only current green main, squash-only, no force/delete")
    return audit_command(argparse.Namespace(json=False))


def audit_repo(root: Path) -> Dict[str, List[str]]:
    findings: Dict[str, List[str]] = {"errors": [], "warnings": [], "ok": []}
    errors, warnings, ok = findings["errors"], findings["warnings"], findings["ok"]

    if shutil.which("gh"):
        repo = gh_json(("api", f"repos/{REPOSITORY}"), cwd=root)
        heads = remote_heads(root)
        main_sha = heads.get(DEFAULT_BRANCH, "")
        if repo.get("allow_merge_commit") or repo.get("allow_rebase_merge"):
            errors.append("repository permits non-squash merge methods")
        else:
            ok.append("squash is the only enabled merge method")
        if not repo.get("delete_branch_on_merge"):
            errors.append("merged branches are not deleted automatically")
        else:
            ok.append("merged branches are deleted automatically")

        _, rulesets = gh_api_optional(f"repos/{REPOSITORY}/rulesets")
        active = [row for row in (rulesets or []) if row.get("enforcement") == "active"]
        protection_code, protection = gh_api_optional(
            f"repos/{REPOSITORY}/branches/{DEFAULT_BRANCH}/protection"
        )
        if not active and protection_code:
            errors.append("no active GitHub ruleset or branch protection protects main")
        else:
            ok.append("GitHub protects main")
        if protection:
            checks = set((protection.get("required_status_checks") or {}).get("contexts") or [])
            required = {"cargo-test", "collaboration-policy"}
            if not required.issubset(checks):
                errors.append("main does not require both cargo-test and collaboration-policy")
            elif not (protection.get("required_status_checks") or {}).get("strict"):
                errors.append("main allows stale PR heads to merge")
            else:
                ok.append("main requires current cargo-test and collaboration-policy checks")
            if not (protection.get("enforce_admins") or {}).get("enabled"):
                errors.append("main protection does not include administrators")
            if not (protection.get("required_conversation_resolution") or {}).get("enabled"):
                errors.append("main does not require conversation resolution")
            if (protection.get("allow_force_pushes") or {}).get("enabled"):
                errors.append("main permits force pushes")
            if (protection.get("allow_deletions") or {}).get("enabled"):
                errors.append("main permits deletion")

        workflows = gh_json(
            ("workflow", "list", "--repo", REPOSITORY, "--json", "name,path,state"),
            cwd=root,
        )
        active_paths = {
            row["path"] for row in (workflows or []) if row.get("state") == "active"
        }
        required_workflows = {
            ".github/workflows/tests.yml",
            ".github/workflows/collaboration-policy.yml",
        }
        missing = sorted(required_workflows - active_paths)
        if missing:
            errors.append("required workflows are not active: " + ", ".join(missing))
        else:
            ok.append("test and collaboration-policy workflows are active")

        prs = existing_pr_claims(root)
        open_heads = {str(pr.get("headRefName") or "") for pr in prs}
        pr_views: Dict[int, Dict[str, Any]] = {}
        pr_changed: Dict[int, Set[str]] = {}
        for pr in prs:
            number = int(pr["number"])
            view = gh_json(
                (
                    "pr",
                    "view",
                    str(number),
                    "--repo",
                    REPOSITORY,
                    "--json",
                    "files,commits,isDraft,body,headRefName,headRefOid,number",
                ),
                cwd=root,
            )
            pr_views[number] = view
            pr_changed[number] = {row["path"] for row in view.get("files", [])}
        for number, view in pr_views.items():
            files = sorted(pr_changed[number])
            subjects = [row["messageHeadline"] for row in view.get("commits", [])]
            others = {key: value for key, value in pr_changed.items() if key != number}
            violations = validate_pr(
                view,
                files=files,
                commit_subjects=subjects,
                other_files=others,
            )
            for violation in violations:
                errors.append(f"PR #{number}: {violation}")
            if not view.get("isDraft") and main_sha:
                head_sha = str(view.get("headRefOid") or "")
                if head_sha:
                    comparison = gh_json(
                        (
                            "api",
                            f"repos/{REPOSITORY}/compare/{main_sha}...{head_sha}",
                        ),
                        cwd=root,
                    )
                    if not compare_status_is_current(
                        str(comparison.get("status") or "")
                    ):
                        errors.append(
                            f"PR #{number}: ready branch does not include current main"
                        )
        if prs and not any(item.startswith("PR #") for item in errors):
            ok.append(f"all {len(prs)} open PR claim(s) satisfy policy")

        for branch in sorted(heads):
            if branch == DEFAULT_BRANCH:
                continue
            if not BRANCH_RE.fullmatch(branch):
                errors.append(f"nonconforming remote development branch: {branch}")
            elif branch not in open_heads:
                errors.append(f"remote task branch has no open PR claim: {branch}")
    else:
        errors.append("GitHub CLI is unavailable; remote enforcement cannot be audited")

    worktrees = parse_worktrees(git(root, "worktree", "list", "--porcelain"))
    for row in worktrees:
        path_text = row.get("worktree", "")
        branch = row.get("branch", "").removeprefix("refs/heads/")
        if "prunable" in row:
            warnings.append(f"prunable worktree registration: {path_text}")
            continue
        path = Path(path_text)
        status = git(path, "status", "--porcelain", check=False) if path.exists() else ""
        if branch == DEFAULT_BRANCH and status:
            errors.append(f"main worktree is dirty: {path}")
        elif branch and branch != DEFAULT_BRANCH and not BRANCH_RE.fullmatch(branch):
            label = "dirty" if status else "clean"
            warnings.append(f"legacy/nonconforming {label} worktree branch {branch}: {path}")
    ok.append(f"inspected {len(worktrees)} local worktree registration(s)")

    hook_error = push_guard_error(root)
    if hook_error:
        errors.append(hook_error)
    else:
        ok.append("shared local pre-push guard is installed and current")

    if sys.platform == "darwin":
        agents = Path.home() / "Library" / "LaunchAgents"
        active = sorted(agents.glob("*civvis*autosync*.plist")) if agents.exists() else []
        if active:
            errors.append("mutating CIVVIS launch agents remain installed: " + ", ".join(map(str, active)))
        else:
            ok.append("no mutating CIVVIS launch agent is installed")
    elif os.name == "nt" and shutil.which("schtasks"):
        result = run(("schtasks", "/Query", "/TN", "CIVVIS Git Autosync"), check=False)
        if result.returncode == 0:
            errors.append("mutating CIVVIS scheduled task remains installed")
    elif shutil.which("systemctl"):
        result = run(("systemctl", "--user", "is-enabled", "civvis-autosync.timer"), check=False)
        if result.returncode == 0:
            errors.append("mutating CIVVIS systemd timer remains enabled")

    return findings


def print_findings(findings: Dict[str, List[str]], *, as_json: bool = False) -> None:
    if as_json:
        print(json.dumps(findings, indent=2, sort_keys=True))
        return
    labels = {"errors": "ERROR", "warnings": "WARNING", "ok": "OK"}
    for level in ("errors", "warnings", "ok"):
        for message in findings[level]:
            print(f"{labels[level]:7} {message}")
    print(
        f"SUMMARY errors={len(findings['errors'])} "
        f"warnings={len(findings['warnings'])} ok={len(findings['ok'])}"
    )


def audit_command(args: argparse.Namespace) -> int:
    findings = audit_repo(repo_root())
    print_findings(findings, as_json=args.json)
    return 1 if findings["errors"] else 0


def monitor_command(args: argparse.Namespace) -> int:
    root = repo_root()
    duration = max(1, args.duration_minutes) * 60
    interval = max(60, args.interval_seconds)
    deadline = time.monotonic() + duration
    log_path = Path(args.log).expanduser().resolve()
    log_path.parent.mkdir(parents=True, exist_ok=True)
    rounds = 0
    ever_failed = False
    previous_heads = remote_heads(root)
    while True:
        rounds += 1
        observed = dt.datetime.now(dt.timezone.utc).isoformat()
        findings = audit_repo(root)
        current_heads = remote_heads(root)
        main_before = previous_heads.get(DEFAULT_BRANCH)
        main_after = current_heads.get(DEFAULT_BRANCH)
        if main_before and main_after and main_before != main_after:
            git(root, "fetch", "--prune", "origin", DEFAULT_BRANCH)
            ancestry = run(
                (
                    "git",
                    "-C",
                    str(root),
                    "merge-base",
                    "--is-ancestor",
                    main_before,
                    main_after,
                ),
                check=False,
            )
            if ancestry.returncode:
                findings["errors"].append(
                    f"main was rewritten or force-pushed: {main_before[:7]} -> {main_after[:7]}"
                )
                commits = [main_after]
            else:
                commits = git(
                    root, "rev-list", "--reverse", f"{main_before}..{main_after}"
                ).splitlines()
            for sha in commits:
                pr_number = associated_pr_number(sha)
                if pr_number is None:
                    subject = git(root, "show", "-s", "--format=%s", sha)
                    findings["errors"].append(
                        f"direct main commit detected: {sha[:7]} {subject}"
                    )
                else:
                    findings["ok"].append(
                        f"main commit {sha[:7]} is backed by merged PR #{pr_number}"
                    )
                    parent_sha = git(root, "rev-parse", f"{sha}^")
                    gate_errors = merged_pr_gate_errors(pr_number, parent_sha)
                    for error in gate_errors:
                        findings["errors"].append(
                            f"PR #{pr_number} merged without a green gate: {error}"
                        )
                    if not gate_errors:
                        findings["ok"].append(
                            f"PR #{pr_number} had both required checks green before merge"
                        )
        for branch, sha in current_heads.items():
            if branch == DEFAULT_BRANCH or previous_heads.get(branch) == sha:
                continue
            if not BRANCH_RE.fullmatch(branch):
                findings["errors"].append(
                    f"new or updated nonconforming remote branch: {branch} at {sha[:7]}"
                )
        previous_heads = current_heads
        ever_failed = ever_failed or bool(findings["errors"])
        record = {"observed_at": observed, "round": rounds, **findings}
        with log_path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(record, sort_keys=True) + "\n")
        print(
            f"MONITOR {observed} round={rounds} errors={len(findings['errors'])} "
            f"warnings={len(findings['warnings'])}",
            flush=True,
        )
        if time.monotonic() >= deadline:
            break
        next_audit = min(deadline, time.monotonic() + interval)
        while time.monotonic() < next_audit:
            remaining = next_audit - time.monotonic()
            time.sleep(min(60, max(0, remaining)))
            if time.monotonic() < next_audit:
                print("MONITOR heartbeat: waiting for next fleet audit", flush=True)
    print(f"MONITOR complete rounds={rounds} log={log_path}")
    return 1 if ever_failed else 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    start = sub.add_parser("start", help="create a task worktree, branch, and draft PR claim")
    start.add_argument("task", help="lowercase hyphenated task slug")
    start.add_argument("--machine", help="stable fleet-unique machine ID")
    start.add_argument("--agent", help="active agent/session ID")
    start.add_argument("--path", action="append", required=True, help="claimed path or glob")
    start.add_argument("--coordinate", type=int, action="append", default=[], help="coordinated PR number")
    start.add_argument("--title", help="draft PR title")
    start.add_argument("--parent", help="directory in which to create the worktree")
    start.add_argument("--dry-run", action="store_true")
    start.set_defaults(func=start_task)

    ship = sub.add_parser(
        "ship",
        help="push a finished task, wait for green CI, merge it, and verify live",
    )
    ship.add_argument("--timeout-seconds", type=float, default=1200.0)
    ship.add_argument("--poll-seconds", type=float, default=10.0)
    ship.add_argument("--live-timeout-seconds", type=float, default=600.0)
    ship.add_argument(
        "--live-url",
        default=os.environ.get(
            "CIVVIS_LIVE_STATUS_URL", "http://127.0.0.1:8766/status"
        ),
    )
    ship.set_defaults(func=ship_task)

    check_pr = sub.add_parser("check-pr", help="validate the current GitHub pull request event")
    check_pr.add_argument("--event", default=os.environ.get("GITHUB_EVENT_PATH", ""))
    check_pr.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY", REPOSITORY))
    check_pr.add_argument("--token", default=os.environ.get("GITHUB_TOKEN", ""))

    audit = sub.add_parser("audit", help="audit local and GitHub fleet enforcement")
    audit.add_argument("--json", action="store_true")
    audit.set_defaults(func=audit_command)

    enforce = sub.add_parser("enforce-github", help="apply repository and main protection settings")
    enforce.set_defaults(func=enforce_github_command)

    install_hooks = sub.add_parser(
        "install-hooks", help="install the shared local pre-push guard for this clone"
    )
    install_hooks.set_defaults(func=install_hooks_command)

    monitor = sub.add_parser("monitor", help="run recurring fleet audits")
    monitor.add_argument("--duration-minutes", type=int, default=180)
    monitor.add_argument("--interval-seconds", type=int, default=300)
    monitor.add_argument(
        "--log",
        default=str(Path.home() / ".local/state/civvis-collab/monitor.jsonl"),
    )
    monitor.set_defaults(func=monitor_command)
    return parser


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.command == "check-pr":
        if not args.event:
            parser.error("check-pr requires --event or GITHUB_EVENT_PATH")
        if not args.token:
            parser.error("check-pr requires --token or GITHUB_TOKEN")
        return check_pr_action(Path(args.event), args.token, args.repository)
    try:
        return int(args.func(args))
    except CommandError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
