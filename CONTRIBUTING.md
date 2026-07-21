# Contributing

- Zero runtime dependencies is a hard rule for the engine package.
- All game content changes go in `civvis/data/*.json`, not code.
- All state mutation goes through `Game.apply`; new actions need: a handler,
  `legal_actions` coverage, and a test.
- Run `python -m pytest` before pushing; keep the suite under a minute.
- Determinism is sacred: any randomness must come from `game.rng` or a seeded
  AI-local RNG.

CI: the workflow file is staged in `ci/tests.yml`; copy it to
`.github/workflows/` (needs a token with workflow scope or the GitHub UI).
