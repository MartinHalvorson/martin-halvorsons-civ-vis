"""Gym-style headless environment: the agent controls player 0, scripted AIs
play the rest. Observations and actions are plain JSON-able dicts."""
from . import hexgrid
from .ai import make_ai
from .game import Game, IllegalAction


class CivEnv:
    def __init__(self, num_players=2, width=20, height=14, seed=0, max_turns=300,
                 opponent="basic", reward_mode="win", num_city_states=0):
        self.num_players = num_players
        self.num_city_states = num_city_states
        self.width = width
        self.height = height
        self.seed = seed
        self.max_turns = max_turns
        self.opponent = opponent
        self.reward_mode = reward_mode
        self.game = None
        self._ais = {}

    # ------------------------------------------------------------------- api

    def reset(self, seed=None):
        if seed is not None:
            self.seed = seed
        self.game = Game(num_players=self.num_players, width=self.width,
                         height=self.height, seed=self.seed,
                         max_turns=self.max_turns,
                         num_city_states=self.num_city_states)
        self._ais = {p.id: make_ai("basic" if p.is_minor else self.opponent,
                                   seed=self.seed + p.id)
                     for p in self.game.players if p.id != 0}
        return self.observe()

    @property
    def done(self):
        return self.game.winner is not None or not self.game.players[0].alive

    def legal_actions(self):
        return self.game.legal_actions(0)

    def step(self, action):
        g = self.game
        info = {}
        prev_score = g.score(0)
        if self.done:
            return self.observe(), 0.0, True, {"already_done": True}
        try:
            g.apply(0, action)
        except IllegalAction as e:
            info["illegal"] = str(e)
            return self.observe(), 0.0, self.done, info
        if action.get("type") == "end_turn":
            self._run_others()
        reward = self._reward(prev_score)
        return self.observe(), reward, self.done, info

    def _run_others(self):
        g = self.game
        guard = 0
        while (g.winner is None and g.current != 0
               and g.players[0].alive and guard < 2 * len(g.players)):
            pid = g.current
            self._ais[pid].take_turn(g, pid)
            if g.current == pid and g.winner is None:
                g.apply(pid, {"type": "end_turn"})
            guard += 1

    def _reward(self, prev_score):
        g = self.game
        terminal = 0.0
        if g.winner == 0:
            terminal = 1.0
        elif g.winner is not None or not g.players[0].alive:
            terminal = -1.0
        if self.reward_mode == "score":
            return (g.score(0) - prev_score) / 100.0 + terminal
        return terminal

    # ----------------------------------------------------------- observation

    def observe(self, pid=0):
        g = self.game
        p = g.players[pid]
        vis = set()
        for u in g.player_units(pid):
            vis.update(hexgrid.disk(u.pos, 2))
        for c in g.player_cities(pid):
            vis.update(hexgrid.disk(c.pos, 2))
            vis.update(c.owned_tiles)
        tiles = []
        for pos in sorted(p.explored):
            t = g.map.get(pos)
            if t is None:
                continue
            oc = g.cities.get(t.owner_city) if t.owner_city is not None else None
            tiles.append({"pos": list(pos), "terrain": t.terrain, "feature": t.feature,
                          "hills": t.hills, "resource": t.resource,
                          "improvement": t.improvement, "district": t.district,
                          "owner": oc.owner if oc else None})
        units = [u.to_dict() for u in g.units.values()
                 if u.owner == pid or u.pos in vis]
        cities = []
        for c in g.cities.values():
            if c.pos not in p.explored:
                continue
            d = {"id": c.id, "name": c.name, "owner": c.owner, "pos": list(c.pos),
                 "pop": c.pop, "hp": c.hp, "is_capital": c.is_capital}
            if c.owner == pid:
                d.update({"food": c.food, "production": c.production,
                          "queue": c.queue, "buildings": list(c.buildings),
                          "districts": {k: list(v) for k, v in c.districts.items()},
                          "owned_tiles": [list(t) for t in c.owned_tiles]})
            cities.append(d)
        return {
            "turn": g.turn,
            "player": pid,
            "current": g.current,
            "map": {"width": g.map.width, "height": g.map.height, "tiles": tiles},
            "units": units,
            "cities": cities,
            "me": {"gold": p.gold, "faith": p.faith,
                   "techs": sorted(p.techs), "research": p.research,
                   "research_progress": p.research_progress,
                   "civics": sorted(p.civics), "civic": p.civic,
                   "civic_progress": p.civic_progress},
            "players": [{"id": o.id, "civ": o.civ, "alive": o.alive,
                         "is_minor": o.is_minor,
                         "score": g.score(o.id),
                         "cities": len(g.player_cities(o.id)),
                         "at_war_with_me": g.is_at_war(pid, o.id)}
                        for o in g.players],
            "winner": g.winner,
            "victory_type": g.victory_type,
        }
