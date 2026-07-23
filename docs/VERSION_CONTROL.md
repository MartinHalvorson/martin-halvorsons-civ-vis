# Version control for the CIVVIS agent fleet

CIVVIS uses protected, PR-based trunk development. The workflow is designed
for any number of computers and concurrent agents. GitHub is the coordination
boundary: local disks hold isolated worktrees, remote task branches hold
checkpoints, draft PRs advertise ownership, and `main` contains only integrated
work.

## The invariants

1. `origin/main` is the only integration trunk and is always expected to build.
2. One task has one new branch, one worktree, one draft PR, and one active
   writer.
3. No computer automatically commits or integrates a development checkout.
4. Every merge reaches `main` through a current, green PR and is squash-merged.
5. Ownership is visible before substantial editing, not discovered at merge
   time.

These rules solve different problems. Worktrees prevent agents on one computer
from sharing uncommitted state. Unique branches prevent computers from pushing
over each other. Draft PRs reveal likely file overlap. Required CI and a single
trunk serialize integration.

## Identities and names

Give each computer a stable, short machine ID such as `martin-mbp` or
`render-win-02`. It must remain unique as the fleet grows. Give each active
agent/session a short ID such as `codex-47` or `claude-ui-3`.

Branches use:

```text
agent/<machine-id>/<agent-id>/<task>-<YYYYMMDDTHHMMSSZ>-<nonce>
```

Example:

```text
agent/martin-mbp/codex-47/mobile-cinema-20260723T210500Z-a31f
```

The nonce can be four or more random hexadecimal characters. A branch name is
never reused, including after its PR is merged. Avoid persistent branches named
only `fix/foo`, `session-x`, or `agent/name`; their ownership and lifetime are
ambiguous.

## Start a task

Start from a stable base checkout used only to manage worktrees. Do not edit in
that checkout. The supported cross-platform launcher performs the fetch,
identity validation, worktree/branch creation, empty claim commit, push, and
draft PR creation:

```bash
python3 tools/civvis_collab.py start mobile-cinema \
  --machine martin-mbp --agent codex-47 \
  --path web/index.html --title "Improve the mobile cinema layout"
```

The launcher records the stable machine ID in repository-local Git config. On
later tasks from that computer, `--machine` may be omitted. If a claim overlaps
an open PR, the launcher refuses to start until coordination is recorded; after
coordinating in the older PR, rerun with `--coordinate <PR-number>`.

The manual equivalent is documented below for recovery and inspection.

```bash
git fetch --prune origin
git worktree add -b \
  agent/martin-mbp/codex-47/mobile-cinema-20260723T210500Z-a31f \
  ../civvis-mobile-cinema-a31f origin/main
cd ../civvis-mobile-cinema-a31f
git status --short --branch
```

Use equivalent PowerShell commands on Windows; the branch and worktree model is
the same on every operating system. Before editing, inspect active ownership:

```bash
gh pr list --repo MartinHalvorson/CIVVIS --state open \
  --json number,isDraft,headRefName,title,url
```

Create a visible claim immediately. An empty claim commit is acceptable because
the PR will eventually be squash-merged:

```bash
git commit --allow-empty -m "claim: mobile cinema layout"
git push -u origin HEAD
gh pr create --draft --base main \
  --title "Improve the mobile cinema layout" \
  --body-file .github/pull_request_template.md
```

Edit the PR body immediately. Fill in machine ID, agent ID, task, exact claimed
paths/globs, dependencies, and any overlap with another PR. The template
is a starting form, not a completed claim.

If another open PR owns an overlapping file or subsystem, use one of these
explicit outcomes before working:

- split the tasks so their paths and responsibilities no longer overlap;
- record which agent owns each hunk or interface in both PRs;
- make the later task wait for the earlier PR, then create a fresh branch from
  the new `origin/main`;
- explicitly hand the entire task to one writer.

Silence is not coordination. Starting anyway merely postpones the collision.

## Work and checkpoint

At the beginning and end of each work period:

```bash
git status --short --branch
git diff --check
git add -- path/to/file another/path
git commit -m "Describe one coherent change"
git push
```

Rules during development:

- Stage only the files belonging to the task. Never sweep the worktree with
  `git add -A` or `git add .`.
- Push useful checkpoints before a context switch, shutdown, or handoff. WIP
  commits are fine on a task branch; squash merge keeps `main` concise.
- Do not use a periodic autosync service as a backup. It cannot tell which
  agent owns a change, whether the change is complete, or what commit message
  describes it.
- Do not rebase or force-push a published branch. Stable history lets another
  computer resume it safely and makes review comments durable.
- Keep the PR narrow. Unrelated changes get their own branch and claim.
- Avoid whole-repository formatting. Run formatters in check mode unless the
  task explicitly owns the resulting files.

Recommended repository-local Git settings are:

```bash
git config fetch.prune true
git config pull.ff only
git config push.default simple
git config merge.conflictStyle zdiff3
git config rerere.enabled true
git config rerere.autoupdate false
```

`rerere` may offer a previous resolution, but `autoupdate=false` keeps it from
staging that resolution without review.

## Move work between computers or agents

A branch still has one writer even when it moves. The current writer must:

1. commit and push every intended change;
2. post a PR comment containing the last commit SHA and the new machine/agent;
3. stop editing and state that the handoff is complete.

Only then may the new writer create its own worktree:

```bash
git fetch origin
git worktree add --track \
  -b agent/martin-mbp/codex-47/mobile-cinema-20260723T210500Z-a31f \
  ../civvis-mobile-cinema-handoff \
  origin/agent/martin-mbp/codex-47/mobile-cinema-20260723T210500Z-a31f
```

On another clone, retaining the exact remote branch name keeps ordinary pushes
safe. For a handoff inside the same clone, remove the old worktree first, then
attach that existing local branch to the new worktree. Do not leave both writers
active.

## Update and integrate

Do not continually merge `main` into every task. Update once when upstream is
needed or just before the PR becomes ready:

```bash
git fetch origin main
git merge --no-edit origin/main
```

Resolve conflicts by intent. Review all three sides with the `zdiff3` context,
run focused tests, and inspect the final diff. Never accept an entire large file
as `ours` or `theirs` merely to clear the index.

Before marking the PR ready:

```bash
git diff --check origin/main...
cargo test --release --locked
```

Also run the soak validation required by `CONTRIBUTING.md` for engine changes.
Record exact commands and results in the PR. CI is a required independent gate,
not a substitute for focused local tests.

Use squash merge only. The squash commit is the atomic unit integrated into
`main`; intermediate checkpoints and merge-from-main commits remain out of the
trunk history. Delete the remote branch after merge. Then remove the worktree
from the base checkout:

```bash
git worktree remove ../civvis-mobile-cinema-a31f
git fetch --prune origin
```

Because squash merge does not make the task commit an ancestor of `main`, Git
may refuse safe local deletion with `git branch -d`. After verifying that the
PR is merged and the remote branch was deleted, remove the remaining local ref
with `git branch -D <exact-task-branch>`.

## GitHub repository settings

The repository owner should enforce the workflow on `main` with a branch
ruleset:

- require a pull request before merging;
- require the `cargo-test` status check and require branches to be current;
- require conversation resolution;
- block force-pushes and branch deletion;
- allow squash merge only;
- automatically delete merged branches;
- optionally enable auto-merge after the required gates pass.

Both `cargo-test` and `collaboration-policy` are required checks. The latter
rejects ambiguous branch names, missing or mismatched machine/agent identity,
changes outside claimed paths, undeclared file overlap with another open PR,
autosync commits, and ready PRs with incomplete validation checkboxes. Run the
same fleet audit locally at any time:

```bash
python3 tools/civvis_collab.py audit
```

After authenticating the repository-owner account, the desired GitHub settings
can be applied and verified without clicking through the UI:

```bash
python3 tools/civvis_collab.py enforce-github
```

For a migration or incident window, run recurring audits into a durable JSONL
log. The monitor audits every five minutes and prints a heartbeat at least once
per minute while waiting:

```bash
python3 tools/civvis_collab.py monitor --duration-minutes 180 \
  --log ~/.local/state/civvis-collab/monitor.jsonl
```

Zero mandatory human approvals is reasonable while autonomous agents are the
primary contributors; CI and current-branch requirements still prevent a
stale concurrent merge. Add an approval requirement later when there are
independent reviewers available. Admin bypass should be reserved for emergency
recovery, not routine integration.

## Automated services

Build, test, spectator, and deployment processes are consumers of Git, not
authors. They may fetch `origin/main` and build it in a private detached
worktree. They must never stage, commit, pull, merge, rebase, reset, or push a
development checkout.

The spectator supervisor already follows the right shape: it builds canonical
`origin/main` in a private worktree and preserves active developer checkouts.
Keep runtime files and generated outputs ignored and local.

## Hotspots and conflict reduction

Several files aggregate many responsibilities and therefore need explicit PR
ownership:

- `src/game.rs`
- `src/ai.rs` and `src/ai/advanced.rs`
- `web/index.html`
- shared tables in `data/*.json`
- broad reference documents such as `README.md` and `docs/MECHANICS.md`

Only one active PR should own broad changes to one of these files unless both
PRs document non-overlapping hunks and an integration order. Long term, split
the largest source and UI files along stable module boundaries; Git workflow
can control concurrency but cannot eliminate collisions inside monoliths.

## Fleet migration and incident recovery

When adopting this workflow on machines that already contain active work:

1. Stop mutating autosync services and pause new edits.
2. Inventory every checkout with `git status --short --branch` and
   `git worktree list` without discarding anything.
3. Give each dirty task an owner. Commit and push it to a uniquely named
   recovery branch, preserving the old branch with an `archive/` tag if needed.
4. Do not merge a large catch-all or autosync branch into `main`. Extract each
   coherent change onto a fresh branch from current `origin/main` and open a
   focused draft PR.
5. Land this workflow and CI, enable the GitHub ruleset, and have every active
   agent reread `AGENTS.md` before work resumes.

If conflict markers or an in-progress merge/rebase already exist, stop all
automated Git writers first. Preserve the worktree and coordinate a single
owner for recovery. Never reset a dirty checkout just to make synchronization
green.
