#!/usr/bin/env python3
"""Fail-closed pre-push policy for CIVVIS development clones."""

from __future__ import annotations

import re
import subprocess
import sys
from typing import Callable, Optional, Sequence


PUSH_GUARD_MARKER = "CIVVIS managed pre-push guard v1"
BRANCH_RE = re.compile(
    r"^agent/(?P<machine>[a-z0-9][a-z0-9-]{0,31})/"
    r"(?P<agent>[a-z0-9][a-z0-9-]{0,31})/"
    r"(?P<task>[a-z0-9][a-z0-9-]{0,47})-"
    r"(?P<stamp>\d{8}T\d{6}Z)-(?P<nonce>[a-f0-9]{4,12})$"
)
MAIN_REF = "refs/heads/main"
HEAD_PREFIX = "refs/heads/"


def is_zero_sha(value: str) -> bool:
    return bool(value) and not value.strip("0")


def validate_push_update(
    local_ref: str,
    local_sha: str,
    remote_ref: str,
    remote_sha: str,
    is_ancestor: Callable[[str, str], bool],
) -> Optional[str]:
    """Return a policy violation for one pre-push update, if any."""
    if remote_ref == MAIN_REF:
        return "direct pushes and deletions of main are forbidden; merge a green PR"
    if not remote_ref.startswith(HEAD_PREFIX):
        return None

    # Permit cleanup of legacy task branches. Main deletion was rejected above.
    if is_zero_sha(local_sha):
        return None

    branch = remote_ref.removeprefix(HEAD_PREFIX)
    if not BRANCH_RE.fullmatch(branch):
        return (
            "development branch must match "
            "agent/<machine>/<agent>/<task>-<YYYYMMDDTHHMMSSZ>-<nonce>"
        )

    if is_zero_sha(remote_sha):
        return None
    if not is_ancestor(remote_sha, local_sha):
        return "non-fast-forward task-branch pushes are forbidden; never force-push"
    return None


def git_is_ancestor(older: str, newer: str) -> bool:
    result = subprocess.run(
        ("git", "merge-base", "--is-ancestor", older, newer),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return result.returncode == 0


def main(argv: Optional[Sequence[str]] = None) -> int:
    del argv  # Git supplies remote name/URL as hook arguments; stdin owns updates.
    violations = []
    for line_number, raw in enumerate(sys.stdin, start=1):
        fields = raw.split()
        if len(fields) != 4:
            violations.append(f"malformed pre-push update on stdin line {line_number}")
            continue
        local_ref, local_sha, remote_ref, remote_sha = fields
        violation = validate_push_update(
            local_ref,
            local_sha,
            remote_ref,
            remote_sha,
            git_is_ancestor,
        )
        if violation:
            violations.append(f"{remote_ref}: {violation}")

    for violation in violations:
        print(f"CIVVIS PUSH REJECTED: {violation}", file=sys.stderr)
    if violations:
        print(
            "Use tools/civvis_collab.py start for a claimed task branch. "
            "Do not bypass this guard with --no-verify.",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
