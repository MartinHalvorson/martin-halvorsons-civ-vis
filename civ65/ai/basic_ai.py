"""Heuristic scripted AI: expands, improves, researches, fights.

Note: this AI reads full game state (no fog) — it is a sparring partner /
baseline, not a fair-play agent. See docs/AI_GUIDE.md.
"""
import random

from .. import hexgrid
from ..combat import effective_strength
from ..game import IllegalAction

TECH_PRIORITY = ["pottery", "animal_husbandry", "mining", "writing", "archery",
                 "bronze_working", "currency", "masonry", "irrigation",
                 "iron_working", "mathematics", "construction", "engineering",
                 "education", "machinery"]
CIVIC_PRIORITY = ["code_of_laws", "craftsmanship", "foreign_trade", "early_empire",
                  "state_workforce", "military_tradition", "drama_poetry",
                  "political_philosophy"]
DISTRICT_PRIORITY = ["campus", "commercial_hub", "holy_site", "theater_square"]


class BasicAI:
    def __init__(self, seed=0):
        self.rng = random.Random(seed)

    def take_turn(self, game, pid):
        self._minor = game.players[pid].is_minor
        try:
            self._research(game, pid)
            self._diplomacy(game, pid)
            self._cities(game, pid)
            self._units(game, pid)
        finally:
            if game.winner is None and game.current == pid:
                game.apply(pid, {"type": "end_turn"})

    def _try(self, game, pid, action):
        try:
            game.apply(pid, action)
            return True
        except IllegalAction:
            return False

    # ------------------------------------------------------------- research

    def _research(self, game, pid):
        p = game.players[pid]
        if p.research is None:
            avail = game.available_techs(p)
            if avail:
                pick = next((t for t in TECH_PRIORITY if t in avail), avail[0])
                self._try(game, pid, {"type": "research", "tech": pick})
        if p.civic is None:
            avail = game.available_civics(p)
            if avail:
                pick = next((c for c in CIVIC_PRIORITY if c in avail), avail[0])
                self._try(game, pid, {"type": "civic", "civic": pick})

    # ------------------------------------------------------------ diplomacy

    def _diplomacy(self, game, pid):
        my_power = game.military_power(pid)
        others = [o for o in game.players if o.id != pid and o.alive]
        for o in others:
            if game.is_at_war(pid, o.id) and my_power < 0.6 * game.military_power(o.id):
                self._try(game, pid, {"type": "make_peace", "player": o.id})
        if self._minor:
            return  # city-states never start wars
        at_war = any(game.is_at_war(pid, o.id) for o in others)
        if not at_war and game.turn > 40 and len(game.player_cities(pid)) >= 2 and others:
            weakest = min(others, key=lambda o: game.military_power(o.id))
            if my_power > 1.8 * game.military_power(weakest.id) + 20:
                self._try(game, pid, {"type": "declare_war", "player": weakest.id})

    # --------------------------------------------------------------- cities

    def _cities(self, game, pid):
        p = game.players[pid]
        my_cities = game.player_cities(pid)
        my_units = game.player_units(pid)
        settlers = sum(1 for u in my_units if u.type == "settler")
        builders = sum(1 for u in my_units if u.type == "builder")
        military = sum(1 for u in my_units
                       if game.rules.units[u.type]["class"] == "military")
        for city in my_cities:
            if city.queue:
                continue
            item = self._pick_item(game, pid, city, len(my_cities),
                                   settlers, builders, military)
            if item and self._try(game, pid, {"type": "produce", "city": city.id,
                                              "item": item}):
                if item.get("unit") == "settler":
                    settlers += 1
                elif item.get("unit") == "builder":
                    builders += 1
                elif "unit" in item:
                    military += 1
        # faith-buy builders when banked
        if p.faith >= 120 and builders < len(my_cities) and my_cities:
            self._try(game, pid, {"type": "buy", "city": my_cities[0].id,
                                  "unit": "builder", "currency": "faith"})

    def _best_military(self, game, pid, city):
        best = None
        for name, spec in game.rules.units.items():
            if spec["class"] != "military" or spec.get("domain") == "sea":
                continue
            if not game.can_produce(pid, city, {"unit": name}):
                continue
            power = max(spec.get("strength", 0), spec.get("ranged_strength", 0))
            if best is None or power > best[0]:
                best = (power, name)
        return best[1] if best else None

    def _pick_item(self, game, pid, city, n_cities, settlers, builders, military):
        if military < n_cities:
            m = self._best_military(game, pid, city)
            if m:
                return {"unit": m}
        if (not self._minor and n_cities + settlers < 4 and settlers == 0
                and city.pop >= 2 and game.turn < 150):
            return {"unit": "settler"}
        if builders < (n_cities + 1) // 2:
            return {"unit": "builder"}
        if "monument" not in city.buildings:
            return {"building": "monument"}
        for dname in DISTRICT_PRIORITY:
            if dname in city.districts:
                continue
            spec = game.rules.districts[dname]
            if not game._unlocked(game.players[pid], spec):
                continue
            sites = game.district_sites(city, dname)
            if sites:
                best = max(sites, key=lambda s: (sum(game.district_yields(dname, s).values()), s))
                return {"district": dname, "pos": list(best)}
        buildable = [(game.rules.buildings[b]["cost"], b) for b in game.rules.buildings
                     if game.can_produce(pid, city, {"building": b})]
        if buildable:
            return {"building": min(buildable)[1]}
        m = self._best_military(game, pid, city)
        return {"unit": m} if m else None

    # ---------------------------------------------------------------- units

    def _units(self, game, pid):
        for u in sorted(game.player_units(pid), key=lambda x: x.id):
            if u.id not in game.units:
                continue
            utype = u.type
            for _ in range(8):
                if u.id not in game.units or u.moves_left <= 0:
                    break
                if utype == "settler":
                    if not self._settler_step(game, pid, u):
                        break
                elif utype == "builder":
                    if not self._builder_step(game, pid, u):
                        break
                else:
                    if not self._military_step(game, pid, u):
                        break

    def _step_toward(self, game, pid, u, target):
        opts = [n for n in hexgrid.neighbors(u.pos) if game.can_move(u, n)]
        if not opts or target is None:
            return False
        best = min(opts, key=lambda n: (hexgrid.distance(n, target), n))
        if hexgrid.distance(best, target) >= hexgrid.distance(u.pos, target):
            return False
        return self._try(game, pid, {"type": "move", "unit": u.id, "to": list(best)})

    def _settle_value(self, game, pos):
        total = 0.0
        for p in hexgrid.disk(pos, 1):
            t = game.map.get(p)
            if t is None or t.owner_city is not None:
                continue
            ys = game.rules.tile_yields(t)
            total += ys["food"] * 1.2 + ys["production"] + ys["gold"] * 0.3
        return total

    def _settler_step(self, game, pid, u):
        best = None
        for pos in hexgrid.disk(u.pos, 5):
            t = game.map.get(pos)
            if t is None or game.rules.is_water(t) or not game.rules.is_passable(t):
                continue
            if any(hexgrid.distance(c.pos, pos) < 4 for c in game.cities.values()):
                continue
            if t.owner_city is not None:
                oc = game.cities.get(t.owner_city)
                if oc and oc.owner != pid:
                    continue
            key = (self._settle_value(game, pos) - 0.4 * hexgrid.distance(u.pos, pos), pos)
            if best is None or key > best[0]:
                best = (key, pos)
        if best is None:
            return False
        target = best[1]
        if target == u.pos:
            return self._try(game, pid, {"type": "found_city", "unit": u.id})
        return self._step_toward(game, pid, u, target)

    def _builder_step(self, game, pid, u):
        p = game.players[pid]
        tile = game.map.get(u.pos)
        imps = game.valid_improvements(p, tile)
        if imps:
            return self._try(game, pid, {"type": "improve", "unit": u.id,
                                         "improvement": imps[0]})
        best = None
        for c in game.player_cities(pid):
            for pos in c.owned_tiles:
                t = game.map.get(pos)
                if game.valid_improvements(p, t):
                    d = hexgrid.distance(u.pos, pos)
                    if best is None or (d, pos) < best:
                        best = (d, pos)
        if best is None:
            return False
        return self._step_toward(game, pid, u, best[1])

    def _military_step(self, game, pid, u):
        spec = game.rules.units[u.type]
        enemies_at_war = [o.id for o in game.players
                          if o.id != pid and o.alive and game.is_at_war(pid, o.id)]
        if enemies_at_war:
            # shoot / attack anything in reach, else advance on nearest target
            rs = spec.get("ranged_strength")
            if rs:
                for pos in hexgrid.disk(u.pos, spec.get("range", 1)):
                    if pos == u.pos or game.map.get(pos) is None:
                        continue
                    if self._is_enemy_tile(game, pid, pos, enemies_at_war):
                        return self._try(game, pid, {"type": "ranged", "unit": u.id,
                                                     "target": list(pos)})
            else:
                for pos in hexgrid.neighbors(u.pos):
                    if game.map.get(pos) is None:
                        continue
                    if self._is_enemy_tile(game, pid, pos, enemies_at_war):
                        if self._worth_attacking(game, u, pos):
                            return self._try(game, pid, {"type": "attack", "unit": u.id,
                                                         "target": list(pos)})
            target = self._nearest_enemy(game, pid, u.pos, enemies_at_war)
            return self._step_toward(game, pid, u, target)
        # peace: minors guard home; majors explore, then garrison
        if self._minor:
            cities = game.player_cities(pid)
            if not cities:
                return False
            cap = cities[0].pos
            if hexgrid.distance(u.pos, cap) > 2:
                return self._step_toward(game, pid, u, cap)
            return False
        target = self._nearest_unexplored(game, pid, u.pos)
        if target is None:
            cities = game.player_cities(pid)
            if not cities:
                return False
            target = min(cities, key=lambda c: hexgrid.distance(u.pos, c.pos)).pos
            if target == u.pos:
                return False
        return self._step_toward(game, pid, u, target)

    def _is_enemy_tile(self, game, pid, pos, enemy_ids):
        for o in game.units_at(pos):
            if o.owner in enemy_ids:
                return True
        c = game.city_at(pos)
        return bool(c and c.owner in enemy_ids)

    def _worth_attacking(self, game, u, pos):
        c = game.city_at(pos)
        if c is not None and c.owner != u.owner:
            return True
        spec = game.rules.units[u.type]
        mine = effective_strength(spec.get("strength", 1), u.hp)
        for o in game.units_at(pos):
            ospec = game.rules.units[o.type]
            if ospec["class"] == "military":
                theirs = effective_strength(ospec.get("strength", 1), o.hp)
                return mine >= theirs - 8
        return True  # civilians

    def _nearest_enemy(self, game, pid, pos, enemy_ids):
        best = None
        for c in game.cities.values():
            if c.owner in enemy_ids:
                d = hexgrid.distance(pos, c.pos)
                if best is None or (d, c.pos) < best:
                    best = (d, c.pos)
        for u in game.units.values():
            if u.owner in enemy_ids:
                d = hexgrid.distance(pos, u.pos)
                if best is None or (d, u.pos) < best:
                    best = (d, u.pos)
        return best[1] if best else None

    def _nearest_unexplored(self, game, pid, pos):
        p = game.players[pid]
        best = None
        for tpos in game.map.tiles:
            if tpos in p.explored:
                continue
            d = hexgrid.distance(pos, tpos)
            if best is None or (d, tpos) < best:
                best = (d, tpos)
        return best[1] if best else None
