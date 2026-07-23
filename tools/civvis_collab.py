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
import subprocess
import sys
import time
from typing import Any, Dict, Iterable, List, Optional, Sequence, Set, Tuple
import urllib.error
import urllib.request


REPOSITORY = "MartinHalvorson/CIVVIS"
DEFAULT_BRANCH = "main"
BRANCH_RE = re.compile(
    r"^agent/(?P<machine>[a-z0-9][a-z0-9-]{0,31})/"
    r"(?P<agent>[a-z0-9][a-z0-9-]{0,31})/"
    r"(?P<task>[a-z0-9][a-z0-9-]{0,47})-"
    r"(?P<stamp>\d{8}T\d{6}Z)-(?P<nonce>[a-f0-9]{4,12})$"
)
ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,31}$")
TASK_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,47}$")
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
                    "files,commits,isDraft,body,headRefName,number",
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
        if prs and not any(item.startswith("PR #") for item in errors):
            ok.append(f"all {len(prs)} open PR claim(s) satisfy policy")
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
    while True:
        rounds += 1
        observed = dt.datetime.now(dt.timezone.utc).isoformat()
        findings = audit_repo(root)
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

    check_pr = sub.add_parser("check-pr", help="validate the current GitHub pull request event")
    check_pr.add_argument("--event", default=os.environ.get("GITHUB_EVENT_PATH", ""))
    check_pr.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY", REPOSITORY))
    check_pr.add_argument("--token", default=os.environ.get("GITHUB_TOKEN", ""))

    audit = sub.add_parser("audit", help="audit local and GitHub fleet enforcement")
    audit.add_argument("--json", action="store_true")
    audit.set_defaults(func=audit_command)

    enforce = sub.add_parser("enforce-github", help="apply repository and main protection settings")
    enforce.set_defaults(func=enforce_github_command)

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
