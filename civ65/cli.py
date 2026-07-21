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


def cmd_simulate(args):
    game = Game(num_players=args.players, width=args.width, height=args.height,
                seed=args.seed, max_turns=args.turns)
    names = args.ai.split(",")
    ais = {p.id: make_ai(names[p.id % len(names)], seed=args.seed + p.id)
           for p in game.players}
    t0 = time.time()
    run_game(game, ais, verbose=not args.quiet, render_every=args.render_every)
    dt = time.time() - t0
    w = game.players[game.winner]
    print(f"\nWinner: {w.civ} (player {w.id}) by {game.victory_type} "
          f"on turn {game.turn}  [{dt:.1f}s]")
    for p in sorted(game.players, key=lambda p: -game.score(p.id)):
        cities = game.player_cities(p.id)
        print(f"  {p.civ:<10} score={game.score(p.id):<4} cities={len(cities)} "
              f"pop={sum(c.pop for c in cities)} techs={len(p.techs)} "
              f"{'' if p.alive else '(eliminated)'}")
    if args.render_every >= 0:
        print()
        print(ascii_map(game))
    if args.save:
        game.save(args.save)
        print(f"saved to {args.save}")


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
    ap = argparse.ArgumentParser(prog="civ65",
                                 description="Headless Civ-6-style strategy engine")
    sub = ap.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("simulate", help="run an AI self-play game")
    s.add_argument("--players", type=int, default=4)
    s.add_argument("--width", type=int, default=28)
    s.add_argument("--height", type=int, default=18)
    s.add_argument("--seed", type=int, default=0)
    s.add_argument("--turns", type=int, default=250)
    s.add_argument("--ai", default="basic", help="comma list cycled over players (basic,random)")
    s.add_argument("--render-every", type=int, default=0)
    s.add_argument("--save", default=None)
    s.add_argument("--quiet", action="store_true")
    s.set_defaults(func=cmd_simulate)

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
