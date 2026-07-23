# The modifier backlog, measured

[FIDELITY.md](FIDELITY.md) phase 1 established that CIVVIS' rules *numbers*
match the shipped game database — 22 tables at zero divergence. That covers
what things cost and what tiles yield. It says nothing about what things *do*.

Civ VI keeps almost all of that in one place. A leader ability, a belief, a
policy card, a governor promotion and a wonder's effect are the same
construction: a row in `Modifiers` naming a `ModifierType`, which
`DynamicModifiers` resolves into an `EffectType` (what happens) and a
`CollectionType` (who it happens to), plus `ModifierArguments` and an optional
`RequirementSet`. CIVVIS instead hardcodes each effect in Rust.

`tools/civ6_modifiers.py` measures what that costs:

```sh
python tools/civ6_modifiers.py                       # ranked report
python tools/civ6_modifiers.py --effect ADJUST_PLOT_YIELD   # every row using one effect
python tools/civ6_modifiers.py --max-unmodelled N    # CI ratchet
```

It shares the rules audit's install detection and load order, and applies the
same baseline exclusions, so the two tools describe the same ruleset.

## What the census says

3,405 modifier rows across **698 distinct effects**, in the Gathering Storm
baseline with optional game modes excluded.

| Status | Effects | Rows |
|---|---:|---:|
| implemented | 25 | 825 |
| partial | 3 | 340 |
| unmodelled | 669 | 2,085 |
| out-of-scope | 1 | 155 |

`tools/modifier_coverage.json` holds those judgements with a reason each.
They are seeded by reading the engine for each effect family, **not** yet
verified row by row — an `implemented` entry is a claim to be checked, and
checking them is the next step. Anything absent from the file counts as
unmodelled, so newly shipped content raises the backlog rather than hiding.

## The finding that matters

The work is not concentrated:

| Share of rows | Effects needed |
|---|---:|
| 50% | 32 |
| 80% | 181 |
| 95% | 528 |
| 100% | 698 |

Thirty-two effects get you half the rows. The remaining half needs another
666, most of which appear two or three times each. That shape is the argument
for phase 2 stated numerically: hardcoding is efficient right up until it
isn't, and the crossover is around the 50% mark, which CIVVIS is already
approaching. Past it, each additional effect buys roughly three rows, and
there is no batch large enough to be worth a bespoke implementation.

The single largest entry says the same thing from the other direction:
`ATTACH_MODIFIER` (336 rows) is the primitive that lets one modifier attach
another to a collection. It is not a game rule at all — it is the
interpreter's own composition operator, and nothing built out of it can be
expressed without building the interpreter.

## Order of work

1. **Verify the 28 implemented and partial effects row by row.** The census is
   only as honest as `modifier_coverage.json`, and those entries are currently
   inspection judgements. Drill in with `--effect`, check each row's arguments
   and requirement set against the CIVVIS path that claims to cover it, and
   demote whatever does not hold.
2. **Close the three `partial` entries.** `ADJUST_PLOT_YIELD`,
   `ADJUST_BUILDING_YIELD_CHANGE` and `GRANT_ABILITY` are 340 rows between
   them, and each is partial for the same reason: a fixed set of named sources
   executes where the game takes an arbitrary one. They are the cheapest
   rehearsal for a general effect table.
3. **Then the interpreter**, in the shape phase 2 of FIDELITY.md describes:
   collections, effects, requirement sets, and a loader that reads the shipped
   `Modifiers` rows rather than transcribing them.

Content scope — the civilizations, units and buildings CIVVIS does not model
at all — is measured separately by the "Only in Civ VI" columns of
`tools/civ6_fidelity.py`. The two backlogs are independent: implementing an
effect makes the content that uses it expressible, and adding content makes
the effects it needs load-bearing.
