# Contributing

- Read [`AGENTS.md`](AGENTS.md) and
  [`docs/VERSION_CONTROL.md`](docs/VERSION_CONTROL.md) before starting. Every
  task uses a unique branch and worktree, one active writer, an early draft PR,
  and squash merge into protected `main`. Development checkouts must not use a
  daemon that stages, commits, pulls, merges, rebases, or pushes.
- Start tasks with `python3 tools/civvis_collab.py start ...`; the launcher
  publishes the ownership claim before implementation begins.
- Pure Rust; serde is the only dependency. Keep it that way.
- All game content changes go in `data/*.json`, not code.
- All state mutation goes through `Game::apply`; new actions need: an
  `Action` variant, a handler, `legal_actions` coverage, and a test.
- Run `cargo test --release` and a `civvis soak` before pushing.
- Determinism is sacred: any randomness must come from `game.rng` or a
  seeded AI-local `Rng`.
- The GUI (`web/index.html`) must only speak the JSON protocol — no
  engine-specific coupling beyond `/state`, `/action`, `/rules`, `/new`.

CI runs `cargo test --release --locked` for every PR and push to `main`.
