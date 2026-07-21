"""Command line interface: headless simulations, benchmarks, save rendering."""
import argparse
import time

from .ai import make_ai
from .game import Game
from .render import ascii_map


def run_game(game, ais, verbose=True, render_every=0):
    last_report = 0
    while game.winner is None:
        pid = game.current
        ais[pid].take_turn(game, pid)
        if game.winner is None and game.current == pid:
            game.apply(pid, {"type": "end_turn"})
        if verbose and game.turn != last_report and game.turn % 25 == 0:
            last_report = game.turn
            scores = " ".join(f"{p.civ}:{game.score(p.id)}" for p in game.players if p.alive)
            print(f"turn {game.turn:4d}  {scores}")
            if render_every and game.turn % render_every == 0:
                print(ascii_map(game))
    return game


def auto_cs(args):
    return args.city_states if args.city_states >= 0 else max(2, args.players // 2)


def cmd_simulate(args):
    game = Game(num_players=args.players, width=args.width, height=args.height,
                seed=args.seed, max_turns=args.turns, num_city_states=auto_cs(args))
    names = args.ai.split(",")
    ais = {p.id: make_ai("basic" if p.is_minor else names[p.id % len(names)],
                         seed=args.seed + p.id)
           for p in game.players}
    t0 = time.time()
    run_game(game, ais, verbose=not args.quiet, render_every=args.render_every)
    dt = time.time() - t0
    w = game.players[game.winner]
    print(f"\nWinner: {w.civ} (player {w.id}) by {game.victory_type} "
          f"on turn {game.turn}  [{dt:.1f}s]")
    for p in sorted(game.players, key=lambda p: -game.score(p.id)):
        if p.is_minor:
            continue
        cities = game.player_cities(p.id)
        print(f"  {p.civ:<10} score={game.score(p.id):<4} cities={len(cities)} "
              f"pop={sum(c.pop for c in cities)} techs={len(p.techs)} "
              f"{'' if p.alive else '(eliminated)'}")
    minors = [p for p in game.players if p.is_minor]
    if minors:
        parts = []
        for p in minors:
            if p.alive:
                parts.append(f"{p.civ} (independent)")
            else:
                cap = next((c for c in game.cities.values()
                            if c.original_owner == p.id), None)
                holder = game.players[cap.owner].civ if cap else "?"
                parts.append(f"{p.civ} (captured by {holder})")
        print("  City-states: " + ", ".join(parts))
    if args.render_every >= 0:
        print()
        print(ascii_map(game))
    if args.save:
        game.save(args.save)
        print(f"saved to {args.save}")


def cmd_soak(args):
    """Play many full AI games across seeds and flag anomalies."""
    cs = auto_cs(args)
    fails, ok = [], 0
    for seed in range(args.start_seed, args.start_seed + args.games):
        t0 = time.time()
        try:
            game = Game(num_players=args.players, width=args.width,
                        height=args.height, seed=seed, max_turns=args.turns,
                        num_city_states=cs)
            ais = {p.id: make_ai("basic", seed=seed * 100 + p.id)
                   for p in game.players}
            run_game(game, ais, verbose=False)
        except Exception as e:  # noqa: BLE001 - soak reports, doesn't crash
            fails.append(f"seed {seed}: {type(e).__name__}: {e}")
            print(f"seed {seed:3d}  CRASH {type(e).__name__}: {e}")
            continue
        majors = [p for p in game.players if not p.is_minor]
        minors = [p for p in game.players if p.is_minor]
        w = game.players[game.winner]
        flags = []
        if sum(len(game.player_cities(p.id)) for p in majors) == 0:
            flags.append("NO-MAJOR-CITIES")
        if all(len(p.techs) <= 2 for p in majors):
            flags.append("NO-TECH-PROGRESS")
        if w.is_minor:
            flags.append("MINOR-WINNER")
        ok += 1
        print(f"seed {seed:3d}  t{game.turn:<4} {game.victory_type:<10} "
              f"{w.civ:<8} majors_alive={sum(p.alive for p in majors)}/{len(majors)} "
              f"cities={len(game.cities):<2} cs_alive={sum(p.alive for p in minors)}/{len(minors)} "
              f"[{time.time() - t0:.1f}s] {' '.join(flags)}")
    print(f"\n{ok}/{args.games} games completed" +
          (f", {len(fails)} FAILED" if fails else ""))
    for f in fails:
        print("  " + f)
    if fails:
        raise SystemExit(1)


def cmd_benchmark(args):
    t0 = time.time()
    game = Game(num_players=2, width=20, height=14, seed=1, max_turns=args.turns)
    ais = {p.id: make_ai("basic", seed=p.id) for p in game.players}
    run_game(game, ais, verbose=False)
    dt = time.time() - t0
    print(f"{game.turn} turns in {dt:.2f}s = {game.turn / dt:.1f} turns/sec "
          f"(2 players, 20x14)")


def cmd_render(args):
    game = Game.load(args.load)
    print(ascii_map(game))


def main(argv=None):
    ap = argparse.ArgumentParser(prog="civvis",
                                 description="Headless Civ-6-style strategy engine")
    sub = ap.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("simulate", help="run an AI self-play game")
    s.add_argument("--players", type=int, default=4)
    s.add_argument("--width", type=int, default=28)
    s.add_argument("--height", type=int, default=18)
    s.add_argument("--seed", type=int, default=0)
    s.add_argument("--turns", type=int, default=250)
    s.add_argument("--ai", default="basic", help="comma list cycled over players (basic,random)")
    s.add_argument("--city-states", type=int, default=-1,
                   help="number of city-states (-1 = auto)")
    s.add_argument("--render-every", type=int, default=0)
    s.add_argument("--save", default=None)
    s.add_argument("--quiet", action="store_true")
    s.set_defaults(func=cmd_simulate)

    k = sub.add_parser("soak", help="run many AI games across seeds, flag anomalies")
    k.add_argument("--games", type=int, default=10)
    k.add_argument("--start-seed", type=int, default=0)
    k.add_argument("--players", type=int, default=4)
    k.add_argument("--width", type=int, default=28)
    k.add_argument("--height", type=int, default=18)
    k.add_argument("--turns", type=int, default=120)
    k.add_argument("--city-states", type=int, default=-1)
    k.set_defaults(func=cmd_soak)

    b = sub.add_parser("benchmark", help="measure engine speed")
    b.add_argument("--turns", type=int, default=100)
    b.set_defaults(func=cmd_benchmark)

    r = sub.add_parser("render", help="print ascii map of a save file")
    r.add_argument("load")
    r.set_defaults(func=cmd_render)

    args = ap.parse_args(argv)
    args.func(args)


if __name__ == "__main__":
    main()
