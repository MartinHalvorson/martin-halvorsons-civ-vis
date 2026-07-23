# CIVVIS agent instructions

These rules apply to every human and automated coding agent in this repository.
Read [docs/VERSION_CONTROL.md](docs/VERSION_CONTROL.md) before changing files.

## Mandatory Git isolation

- Treat `main` as read-only. Never develop on it or push to it directly.
- Every task gets a new branch and a separate Git worktree created from the
  latest `origin/main`.
- A branch and its worktree have exactly one writer. Do not let two agents,
  processes, or computers edit the same branch or worktree concurrently.
- Use a globally unique branch name:
  `agent/<machine-id>/<agent-id>/<task>-<UTC timestamp>-<nonce>`.
  Never reuse a branch for another task or PR.
- Open a draft PR before substantial editing. Its ownership block must name
  the machine, agent, task, and expected paths/components. Check all open draft
  and ready PRs first; overlapping ownership must be coordinated in the older PR before
  either agent continues.
- If work moves to another computer or agent, record an explicit handoff in the
  draft PR, push the current commit, and stop the old writer before the new one
  starts.

## Safe Git behavior

- Before editing, run `git status --short --branch` and confirm that the branch
  is neither `main` nor another task's branch.
- Preserve dirty work you did not create. Never reset, discard, stash, stage,
  commit, or move another writer's changes.
- Stage exact paths with `git add -- <paths>`. Do not use `git add -A` in a
  shared repository.
- Make descriptive checkpoint commits and push them to the task branch. A push
  is the cross-machine backup and handoff mechanism; `git stash` is not.
- Do not force-push. Do not rebase a branch after it has been pushed. To update
  a task branch, fetch and merge `origin/main` into it once, resolve carefully,
  rerun validation, and push normally.
- Never run a repository-wide daemon that stages, commits, pulls, rebases,
  merges, or pushes development work. Automated builds may fetch and use a
  private detached worktree based on `origin/main`; they must not mutate a
  development checkout.
- Do not perform broad formatting, generated-file rewrites, or unrelated
  cleanup in a feature PR. CIVVIS has several large conflict hotspots, notably
  `src/game.rs`, `src/ai.rs`, and `web/index.html`.

## Integration

- Keep tasks small and single-purpose. Split independent work into independent
  branches and PRs.
- Before marking a PR ready, merge the latest `origin/main`, run the validation
  required by `CONTRIBUTING.md`, and record the results in the PR.
- Merge only through a green PR using squash merge. Delete the remote task
  branch after merge and remove the local worktree.
- If a conflict is semantic or ownership is unclear, stop and coordinate. Do
  not resolve a whole file with `--ours` or `--theirs` merely to make Git pass.
