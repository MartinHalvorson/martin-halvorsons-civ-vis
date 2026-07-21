"""Core turn-based engine. Fully headless and deterministic given a seed.

All interaction happens through JSON-friendly action dicts (see legal_actions),
so AIs, RL agents, UIs and network clients all speak the same protocol.
"""
import json
import random

from . import hexgrid, mapgen
from .combat import damage, effective_strength
from .entities import City, Player, Unit
from .rules import Ruleset
from .world import WorldMap, add_yields, zero_yields

CIV_NAMES = ["Rome", "Egypt", "Greece", "China", "Sumeria", "Aztec", "Nubia", "Scythia"]
CITY_NAMES = {
    "Rome": ["Rome", "Ostia", "Antium", "Ravenna"],
    "Egypt": ["Thebes", "Memphis", "Akhetaten", "Giza"],
    "Greece": ["Athens", "Sparta", "Corinth", "Argos"],
    "China": ["Xian", "Chengdu", "Luoyang", "Kaifeng"],
    "Sumeria": ["Uruk", "Ur", "Nippur", "Lagash"],
    "Aztec": ["Tenochtitlan", "Texcoco", "Tlatelolco", "Xochimilco"],
    "Nubia": ["Meroe", "Kerma", "Napata", "Dongola"],
    "Scythia": ["Pokrovka", "Gelonos", "Kamenka", "Aktau"],
}
STARTING_TECHS = {"agriculture"}
CITY_STATE_NAMES = ["Kabul", "Geneva", "Carthage", "Hattusa", "Mohenjo-Daro",
                    "Yerevan", "Zanzibar", "Auckland", "Valletta", "Vilnius",
                    "Stockholm", "Kandy"]


class IllegalAction(Exception):
    pass


def growth_threshold(pop):
    """Food needed to grow from `pop` to `pop + 1` (Civ 6 curve)."""
    return 15 + 8 * (pop - 1) + int((pop - 1) ** 1.5)


class Game:
    def __init__(self, num_players=2, width=24, height=16, seed=0, max_turns=500,
                 ruleset=None, num_city_states=0, _skip_setup=False):
        self.rules = ruleset or Ruleset()
        self.seed = seed
        self.max_turns = max_turns
        self.turn = 1
        self.current = 0
        self.winner = None
        self.victory_type = None
        self.next_id = 1
        self.units = {}
        self.cities = {}
        self.at_war = set()  # of frozenset({a, b})
        self._occ = {}       # pos -> [unit ids]
        self._city_by_pos = {}
        self.rng = random.Random(seed)
        if _skip_setup:
            self.map = None
            self.players = []
            return
        total = num_players + num_city_states
        self.map, spawns = mapgen.generate(self.rules, width, height, total, self.rng)
        self.players = [Player(id=i, civ=CIV_NAMES[i % len(CIV_NAMES)],
                               techs=set(STARTING_TECHS)) for i in range(num_players)]
        for p, pos in zip(self.players, spawns[:num_players]):
            self._spawn_unit("settler", p.id, pos)
            self._spawn_unit("warrior", p.id, pos)
            self._reveal(p, pos, radius=3)
        # city-states: pre-founded single-city minor players
        for i, pos in enumerate(spawns[num_players:]):
            if any(hexgrid.distance(pos, s) < 4 for s in spawns[:num_players]) or \
               any(hexgrid.distance(pos, c.pos) < 4 for c in self.cities.values()):
                continue  # too crowded; skip this city-state
            pid = len(self.players)
            name = CITY_STATE_NAMES[i % len(CITY_STATE_NAMES)]
            p = Player(id=pid, civ=name, techs=set(STARTING_TECHS), is_minor=True)
            self.players.append(p)
            self._found_city_for(p, pos, name=name)
            self._place_new_unit("warrior", pid, pos)
            self._place_new_unit("slinger", pid, pos)

    # ------------------------------------------------------------------ queries

    def city_at(self, pos):
        cid = self._city_by_pos.get(tuple(pos))
        return self.cities.get(cid) if cid is not None else None

    def units_at(self, pos):
        return [self.units[i] for i in self._occ.get(tuple(pos), [])]

    def player_units(self, pid):
        return [u for u in self.units.values() if u.owner == pid]

    def player_cities(self, pid):
        return [c for c in self.cities.values() if c.owner == pid]

    def is_at_war(self, a, b):
        return frozenset((a, b)) in self.at_war

    def available_techs(self, p):
        return sorted(t for t, s in self.rules.techs.items()
                      if t not in p.techs and set(s["requires"]) <= p.techs)

    def available_civics(self, p):
        return sorted(c for c, s in self.rules.civics.items()
                      if c not in p.civics and set(s["requires"]) <= p.civics)

    def score(self, pid):
        p = self.players[pid]
        cities = self.player_cities(pid)
        return (10 * len(cities)
                + 3 * sum(c.pop for c in cities)
                + 3 * sum(len(c.districts) for c in cities)
                + 1 * sum(len(c.buildings) for c in cities)
                + 2 * len(p.techs) + 2 * len(p.civics)
                + 1 * len(self.player_units(pid)))

    def military_power(self, pid):
        return sum(self.rules.units[u.type].get("strength", 0) * u.hp / 100.0
                   for u in self.player_units(pid))

    def _unlocked(self, p, spec):
        if spec.get("tech") and spec["tech"] not in p.techs:
            return False
        if spec.get("civic") and spec["civic"] not in p.civics:
            return False
        return True

    def _has_resource(self, pid, res):
        for c in self.player_cities(pid):
            for pos in c.owned_tiles:
                if self.map.get(pos).resource == res:
                    return True
        return False

    # ------------------------------------------------------------ unit helpers

    def _spawn_unit(self, utype, owner, pos):
        spec = self.rules.units[utype]
        u = Unit(id=self.next_id, type=utype, owner=owner, pos=tuple(pos),
                 moves_left=spec["moves"], charges=spec.get("charges", 0))
        self.next_id += 1
        self.units[u.id] = u
        self._occ.setdefault(u.pos, []).append(u.id)
        self._reveal(self.players[owner], u.pos)
        return u

    def _remove_unit(self, uid):
        u = self.units.pop(uid, None)
        if u:
            ids = self._occ.get(u.pos, [])
            if uid in ids:
                ids.remove(uid)
            if not ids:
                self._occ.pop(u.pos, None)

    def _relocate(self, u, pos):
        ids = self._occ.get(u.pos, [])
        if u.id in ids:
            ids.remove(u.id)
        if not ids:
            self._occ.pop(u.pos, None)
        u.pos = tuple(pos)
        self._occ.setdefault(u.pos, []).append(u.id)
        self._reveal(self.players[u.owner], u.pos)

    def _reveal(self, player, pos, radius=2):
        for p in hexgrid.disk(tuple(pos), radius):
            if p in self.map.tiles:
                player.explored.add(p)

    def can_move(self, unit, pos):
        pos = tuple(pos)
        if hexgrid.distance(unit.pos, pos) != 1:
            return False
        t = self.map.get(pos)
        if t is None or not self.rules.is_passable(t):
            return False
        spec = self.rules.units[unit.type]
        water = self.rules.is_water(t)
        if spec.get("domain") == "sea":
            if not water:
                return False
        elif water:
            return False
        for o in self.units_at(pos):
            ospec = self.rules.units[o.type]
            if o.owner != unit.owner:
                if ospec["class"] == "military" or spec["class"] == "civilian":
                    return False
                if not self.is_at_war(unit.owner, o.owner):
                    return False
            elif ospec["class"] == spec["class"]:
                return False
        c = self.city_at(pos)
        if c and c.owner != unit.owner:
            return False
        return True

    # ------------------------------------------------------------ city helpers

    def can_found_city(self, u):
        t = self.map.get(u.pos)
        if self.rules.is_water(t) or not self.rules.is_passable(t):
            return False
        for c in self.cities.values():
            if hexgrid.distance(c.pos, u.pos) < 4:
                return False
        if t.owner_city is not None:
            oc = self.cities.get(t.owner_city)
            if oc and oc.owner != u.owner:
                return False
        return True

    def city_strength(self, city):
        s = 10 + 2 * city.pop
        if "walls" in city.buildings:
            s += 10
        if "encampment" in city.districts:
            s += self.rules.districts["encampment"].get("defense", 10)
        garrison = [o for o in self.units_at(city.pos)
                    if o.owner == city.owner and self.rules.units[o.type]["class"] == "military"]
        if garrison:
            s += 5
        return s

    def district_yields(self, dname, dpos):
        spec = self.rules.districts[dname]
        ys = zero_yields()
        add_yields(ys, spec.get("yields", {}))
        adj = spec.get("adjacency", {})
        if adj:
            counts = {"mountain": 0, "forest": 0, "district": 0}
            for t in self.map.neighbors(dpos):
                if t.terrain == "mountain":
                    counts["mountain"] += 1
                if t.feature == "forest":
                    counts["forest"] += 1
                if t.district:
                    counts["district"] += 1
            for key, bonus in adj.items():
                for yk, amt in bonus.items():
                    ys[yk] += int(counts.get(key, 0) * amt)
        return ys

    def city_yields(self, city):
        ys = zero_yields()
        center = self.rules.tile_yields(self.map.get(city.pos))
        center["food"] = max(center["food"], 2)
        center["production"] = max(center["production"], 1)
        add_yields(ys, center)
        cands = []
        for pos in city.owned_tiles:
            if pos == city.pos:
                continue
            t = self.map.get(pos)
            if t.district:
                continue
            tys = self.rules.tile_yields(t)
            val = (tys["food"] * 1.5 + tys["production"] * 1.5 + tys["gold"] * 0.7
                   + tys["science"] + tys["culture"] + tys["faith"])
            cands.append((val, pos, tys))
        cands.sort(key=lambda x: (-x[0], x[1]))
        for _, _, tys in cands[:city.pop]:
            add_yields(ys, tys)
        for dname, dpos in city.districts.items():
            add_yields(ys, self.district_yields(dname, dpos))
        for b in city.buildings:
            add_yields(ys, self.rules.buildings[b].get("yields", {}))
        ys["science"] += 0.5 * city.pop
        ys["culture"] += 0.3 * city.pop
        if city.is_capital and city.owner == city.original_owner:
            add_yields(ys, {"gold": 3, "science": 1, "culture": 1})
        return ys

    def valid_improvements(self, p, tile):
        if tile is None or tile.district or self.city_at(tile.pos):
            return []
        if tile.owner_city is None:
            return []
        oc = self.cities.get(tile.owner_city)
        if oc is None or oc.owner != p.id:
            return []
        specs = self.rules.improvements
        out = []
        if tile.resource:
            imp = self.rules.resources[tile.resource]["improvement"]
            spec = specs[imp]
            if self._unlocked(p, spec) and self.rules.is_water(tile) == bool(spec.get("water")):
                if tile.improvement != imp:
                    out.append(imp)
        elif not self.rules.is_water(tile):
            if tile.hills:
                if self._unlocked(p, specs["mine"]) and tile.improvement != "mine":
                    out.append("mine")
            elif tile.terrain in specs["farm"].get("terrain", []):
                if self._unlocked(p, specs["farm"]) and tile.improvement != "farm":
                    out.append("farm")
        return out

    def district_sites(self, city, dname):
        spec = self.rules.districts[dname]
        want_water = bool(spec.get("water"))
        out = []
        for pos in city.owned_tiles:
            if pos == city.pos or hexgrid.distance(pos, city.pos) > 3:
                continue
            t = self.map.get(pos)
            if t.district or t.resource or not self.rules.is_passable(t):
                continue
            if self.rules.is_water(t) != want_water:
                continue
            out.append(pos)
        return sorted(out)

    def item_cost(self, item):
        if "unit" in item:
            return self.rules.units[item["unit"]]["cost"]
        if "building" in item:
            return self.rules.buildings[item["building"]]["cost"]
        return self.rules.districts[item["district"]]["cost"]

    def can_produce(self, pid, city, item):
        p = self.players[pid]
        if "unit" in item:
            spec = self.rules.units.get(item["unit"])
            if not spec or not self._unlocked(p, spec):
                return False
            res = spec.get("requires_resource")
            if res and not self._has_resource(pid, res):
                return False
            if spec.get("domain") == "sea":
                if not any(self.rules.is_water(t) for t in self.map.neighbors(city.pos)):
                    return False
            return True
        if "building" in item:
            spec = self.rules.buildings.get(item["building"])
            if not spec or item["building"] in city.buildings or not self._unlocked(p, spec):
                return False
            need = spec.get("district")
            return need is None or need in city.districts
        if "district" in item:
            spec = self.rules.districts.get(item["district"])
            if not spec or item["district"] in city.districts or not self._unlocked(p, spec):
                return False
            pos = item.get("pos")
            return pos is not None and tuple(pos) in self.district_sites(city, item["district"])
        return False

    def producible_items(self, pid, city):
        p = self.players[pid]
        items = []
        for name in self.rules.units:
            it = {"unit": name}
            if self.can_produce(pid, city, it):
                items.append(it)
        for name in self.rules.buildings:
            it = {"building": name}
            if self.can_produce(pid, city, it):
                items.append(it)
        for name in self.rules.districts:
            if name in city.districts or not self._unlocked(p, self.rules.districts[name]):
                continue
            sites = self.district_sites(city, name)
            if sites:
                ranked = sorted(sites, key=lambda s: (-sum(self.district_yields(name, s).values()), s))
                for s in ranked[:2]:
                    items.append({"district": name, "pos": list(s)})
        return items

    # ------------------------------------------------------------ action layer

    def legal_actions(self, pid):
        if self.winner is not None or self.current != pid:
            return []
        p = self.players[pid]
        acts = []
        for u in sorted(self.player_units(pid), key=lambda x: x.id):
            spec = self.rules.units[u.type]
            if u.moves_left > 0:
                for n in hexgrid.neighbors(u.pos):
                    if self.can_move(u, n):
                        acts.append({"type": "move", "unit": u.id, "to": list(n)})
                if spec["class"] == "military":
                    if spec.get("ranged_strength"):
                        for pos in hexgrid.disk(u.pos, spec.get("range", 1)):
                            if pos == u.pos or pos not in self.map.tiles:
                                continue
                            if self._enemy_target_at(pid, pos):
                                acts.append({"type": "ranged", "unit": u.id, "target": list(pos)})
                    else:
                        for pos in hexgrid.neighbors(u.pos):
                            if pos in self.map.tiles and self._enemy_target_at(pid, pos):
                                acts.append({"type": "attack", "unit": u.id, "target": list(pos)})
            if u.type == "settler" and self.can_found_city(u):
                acts.append({"type": "found_city", "unit": u.id})
            if u.type == "builder" and u.charges > 0:
                for imp in self.valid_improvements(p, self.map.get(u.pos)):
                    acts.append({"type": "improve", "unit": u.id, "improvement": imp})
        for c in self.player_cities(pid):
            for item in self.producible_items(pid, c):
                acts.append({"type": "produce", "city": c.id, "item": item})
            for utype in ("builder", "settler", "warrior", "archer", "spearman"):
                it = {"unit": utype}
                if self.can_produce(pid, c, it):
                    cost = self.rules.units[utype]["cost"]
                    if p.gold >= cost * 4:
                        acts.append({"type": "buy", "city": c.id, "unit": utype, "currency": "gold"})
                    if p.faith >= cost * 2 and utype in ("builder", "settler"):
                        acts.append({"type": "buy", "city": c.id, "unit": utype, "currency": "faith"})
        if p.research is None:
            for t in self.available_techs(p):
                acts.append({"type": "research", "tech": t})
        if p.civic is None:
            for c in self.available_civics(p):
                acts.append({"type": "civic", "civic": c})
        for o in self.players:
            if o.id != pid and o.alive:
                if self.is_at_war(pid, o.id):
                    acts.append({"type": "make_peace", "player": o.id})
                else:
                    acts.append({"type": "declare_war", "player": o.id})
        acts.append({"type": "end_turn"})
        return acts

    def _enemy_target_at(self, pid, pos):
        for o in self.units_at(pos):
            if o.owner != pid:
                return True
        c = self.city_at(pos)
        return bool(c and c.owner != pid)

    def apply(self, pid, action):
        if self.winner is not None:
            raise IllegalAction("game over")
        if self.current != pid:
            raise IllegalAction("not your turn")
        p = self.players[pid]
        handlers = {
            "move": self._do_move, "attack": self._do_attack, "ranged": self._do_ranged,
            "found_city": self._do_found_city, "improve": self._do_improve,
            "produce": self._do_produce, "buy": self._do_buy,
            "research": self._do_research, "civic": self._do_civic,
            "declare_war": self._do_declare_war, "make_peace": self._do_make_peace,
            "end_turn": self._do_end_turn,
        }
        h = handlers.get(action.get("type"))
        if h is None:
            raise IllegalAction(f"unknown action type {action.get('type')!r}")
        h(p, action)

    def _own_unit(self, p, uid):
        u = self.units.get(uid)
        if u is None or u.owner != p.id:
            raise IllegalAction("not your unit")
        return u

    def _own_city(self, p, cid):
        c = self.cities.get(cid)
        if c is None or c.owner != p.id:
            raise IllegalAction("not your city")
        return c

    def _do_move(self, p, action):
        u = self._own_unit(p, action["unit"])
        if u.moves_left <= 0:
            raise IllegalAction("no moves left")
        to = tuple(action["to"])
        if not self.can_move(u, to):
            raise IllegalAction("invalid move")
        for o in self.units_at(to):
            if o.owner != u.owner:
                o.owner = u.owner  # capture undefended civilian
        cost = self.rules.move_cost(self.map.get(to))
        self._relocate(u, to)
        u.moves_left = max(0.0, u.moves_left - cost)

    def _auto_declare_war(self, a, b):
        if a != b and not self.is_at_war(a, b):
            self.at_war.add(frozenset((a, b)))

    def _tile_defense_bonus(self, pos):
        t = self.map.get(pos)
        return 3.0 if (t.hills or t.feature in ("forest", "jungle")) else 0.0

    def _do_attack(self, p, action):
        u = self._own_unit(p, action["unit"])
        spec = self.rules.units[u.type]
        if spec["class"] != "military" or spec.get("ranged_strength"):
            raise IllegalAction("unit cannot melee attack")
        if u.moves_left <= 0:
            raise IllegalAction("no moves left")
        target = tuple(action["target"])
        if hexgrid.distance(u.pos, target) != 1:
            raise IllegalAction("target not adjacent")
        enemies = [o for o in self.units_at(target) if o.owner != u.owner]
        city = self.city_at(target)
        if city is not None and city.owner == u.owner:
            city = None
        if not enemies and city is None:
            raise IllegalAction("nothing to attack")
        for e in enemies:
            self._auto_declare_war(u.owner, e.owner)
        if city is not None:
            self._auto_declare_war(u.owner, city.owner)
        military = [e for e in enemies if self.rules.units[e.type]["class"] == "military"]
        att = effective_strength(spec["strength"], u.hp)
        u.moves_left = 0
        if military:
            d = max(military, key=lambda e: effective_strength(
                self.rules.units[e.type].get("strength", 1), e.hp))
            ds = effective_strength(self.rules.units[d.type].get("strength", 1), d.hp) \
                + self._tile_defense_bonus(target)
            d.hp -= damage(att, ds, self.rng)
            u.hp -= damage(ds, att, self.rng)
            if d.hp <= 0:
                self._remove_unit(d.id)
                self._on_unit_lost(d.owner)
            if u.hp <= 0:
                self._remove_unit(u.id)
                self._on_unit_lost(u.owner)
                return
            if d.hp <= 0 and not any(
                    self.rules.units[e.type]["class"] == "military"
                    for e in self.units_at(target) if e.owner != u.owner):
                if city is None or city.hp <= 0:
                    self._enter_tile(u, target)
        elif city is not None and city.hp > 0:
            cs = self.city_strength(city)
            city.hp -= damage(att, cs, self.rng)
            u.hp -= damage(cs, att, self.rng)
            if u.hp <= 0:
                self._remove_unit(u.id)
                self._on_unit_lost(u.owner)
                city.hp = max(1, city.hp)
                return
            if city.hp <= 0:
                self._capture_city(city, u.owner)
                self._enter_tile(u, target)
        else:
            # only undefended civilians: capture them by entering
            self._enter_tile(u, target)

    def _enter_tile(self, u, pos):
        for o in self.units_at(pos):
            if o.owner != u.owner:
                o.owner = u.owner
        self._relocate(u, pos)

    def _do_ranged(self, p, action):
        u = self._own_unit(p, action["unit"])
        spec = self.rules.units[u.type]
        rs = spec.get("ranged_strength")
        if not rs:
            raise IllegalAction("unit has no ranged attack")
        if u.moves_left <= 0:
            raise IllegalAction("no moves left")
        target = tuple(action["target"])
        if hexgrid.distance(u.pos, target) > spec.get("range", 1):
            raise IllegalAction("out of range")
        enemies = [o for o in self.units_at(target) if o.owner != u.owner]
        city = self.city_at(target)
        if city is not None and city.owner == u.owner:
            city = None
        if not enemies and city is None:
            raise IllegalAction("nothing to attack")
        for e in enemies:
            self._auto_declare_war(u.owner, e.owner)
        if city is not None:
            self._auto_declare_war(u.owner, city.owner)
        att = effective_strength(rs, u.hp)
        u.moves_left = 0
        military = [e for e in enemies if self.rules.units[e.type]["class"] == "military"]
        if military:
            d = max(military, key=lambda e: effective_strength(
                self.rules.units[e.type].get("strength", 1), e.hp))
            ds = effective_strength(self.rules.units[d.type].get("strength", 1), d.hp) \
                + self._tile_defense_bonus(target)
            d.hp -= damage(att, ds, self.rng)
            if d.hp <= 0:
                self._remove_unit(d.id)
                self._on_unit_lost(d.owner)
        elif enemies:
            d = enemies[0]
            d.hp -= damage(att, 1.0, self.rng)
            if d.hp <= 0:
                self._remove_unit(d.id)
                self._on_unit_lost(d.owner)
        else:
            cs = self.city_strength(city)
            city.hp = max(1, city.hp - damage(att, cs, self.rng))

    def _do_found_city(self, p, action):
        u = self._own_unit(p, action["unit"])
        if u.type != "settler":
            raise IllegalAction("only settlers found cities")
        if not self.can_found_city(u):
            raise IllegalAction("cannot found city here")
        self._found_city_for(p, u.pos)
        self._remove_unit(u.id)

    def _found_city_for(self, p, pos, name=None):
        if name is None:
            names = CITY_NAMES.get(p.civ, [])
            n_mine = len([c for c in self.cities.values() if c.original_owner == p.id])
            name = names[n_mine] if n_mine < len(names) else f"{p.civ} {n_mine + 1}"
        city = City(id=self.next_id, name=name, owner=p.id, pos=tuple(pos),
                    original_owner=p.id,
                    is_capital=(not p.is_minor) and not any(
                        c.original_owner == p.id and c.is_capital
                        for c in self.cities.values()))
        self.next_id += 1
        self.cities[city.id] = city
        self._city_by_pos[city.pos] = city.id
        center = self.map.get(city.pos)
        center.feature = None
        center.improvement = None
        for tpos in [city.pos] + hexgrid.neighbors(city.pos):
            t = self.map.get(tpos)
            if t is not None and t.owner_city is None:
                t.owner_city = city.id
                city.owned_tiles.append(tpos)
        self._reveal(p, city.pos, radius=3)
        return city

    def _do_improve(self, p, action):
        u = self._own_unit(p, action["unit"])
        if u.type != "builder" or u.charges <= 0:
            raise IllegalAction("not a builder with charges")
        tile = self.map.get(u.pos)
        imp = action["improvement"]
        if imp not in self.valid_improvements(p, tile):
            raise IllegalAction("invalid improvement here")
        tile.improvement = imp
        if self.rules.improvements[imp].get("removes_feature") and tile.feature:
            tile.feature = None
        u.charges -= 1
        u.moves_left = 0
        if u.charges <= 0:
            self._remove_unit(u.id)

    def _do_produce(self, p, action):
        city = self._own_city(p, action["city"])
        item = dict(action["item"])
        if "pos" in item:
            item["pos"] = list(item["pos"])
        if not self.can_produce(p.id, city, item):
            raise IllegalAction("cannot produce that")
        city.queue = [item]

    def _do_buy(self, p, action):
        city = self._own_city(p, action["city"])
        utype = action["unit"]
        if not self.can_produce(p.id, city, {"unit": utype}):
            raise IllegalAction("cannot buy that")
        if utype == "settler" and city.pop < 2:
            raise IllegalAction("city too small for settler")
        cur = action.get("currency", "gold")
        cost = self.rules.units[utype]["cost"] * (4 if cur == "gold" else 2)
        bank = p.gold if cur == "gold" else p.faith
        if bank < cost:
            raise IllegalAction("cannot afford")
        u = self._place_new_unit(utype, p.id, city.pos)
        if u is None:
            raise IllegalAction("no space to place unit")
        if cur == "gold":
            p.gold -= cost
        else:
            p.faith -= cost
        if utype == "settler":
            city.pop -= 1

    def _do_research(self, p, action):
        if p.research is not None:
            raise IllegalAction("already researching")
        t = action["tech"]
        if t not in self.available_techs(p):
            raise IllegalAction("tech unavailable")
        p.research = t
        p.research_progress = p.research_overflow
        p.research_overflow = 0.0

    def _do_civic(self, p, action):
        if p.civic is not None:
            raise IllegalAction("already working a civic")
        c = action["civic"]
        if c not in self.available_civics(p):
            raise IllegalAction("civic unavailable")
        p.civic = c
        p.civic_progress = p.civic_overflow
        p.civic_overflow = 0.0

    def _do_declare_war(self, p, action):
        o = action["player"]
        if o == p.id or not self.players[o].alive:
            raise IllegalAction("invalid war target")
        self.at_war.add(frozenset((p.id, o)))

    def _do_make_peace(self, p, action):
        o = action["player"]
        pair = frozenset((p.id, o))
        if pair not in self.at_war:
            raise IllegalAction("not at war")
        self.at_war.discard(pair)

    def _do_end_turn(self, p, action=None):
        n = len(self.players)
        nxt = None
        for i in range(1, n + 1):
            cand = (self.current + i) % n
            if self.players[cand].alive:
                nxt = cand
                break
        if nxt is None or nxt == self.current:
            return
        wrapped = nxt <= self.current
        self.current = nxt
        if wrapped:
            self.turn += 1
            if self.turn > self.max_turns and self.winner is None:
                majors = [pl for pl in self.players if pl.alive and not pl.is_minor]
                best = max(majors or self.players,
                           key=lambda pl: (self.score(pl.id), -pl.id))
                self._set_winner(best.id, "score")
        if self.winner is None:
            self._begin_turn(self.players[self.current])

    # ------------------------------------------------------------ turn engine

    def _begin_turn(self, p):
        for u in self.player_units(p.id):
            spec = self.rules.units[u.type]
            u.moves_left = spec["moves"]
            if u.hp < 100:
                t = self.map.get(u.pos)
                own = t.owner_city is not None and \
                    self.cities[t.owner_city].owner == p.id
                u.hp = min(100, u.hp + (15 if own else 10))
        sci = cul = gold = faith = 0.0
        for city in self.player_cities(p.id):
            ys = self._process_city(p, city)
            sci += ys["science"]
            cul += ys["culture"]
            gold += ys["gold"]
            faith += ys["faith"]
        gold -= max(0, len(self.player_units(p.id)) - 3)
        p.gold = max(0.0, p.gold + gold)
        p.faith += faith
        if p.research:
            p.research_progress += sci
            cost = self.rules.techs[p.research]["cost"]
            if p.research_progress >= cost:
                p.techs.add(p.research)
                p.research_overflow = p.research_progress - cost
                p.research = None
                p.research_progress = 0.0
                if not p.is_minor and len(p.techs) >= len(self.rules.techs):
                    self._set_winner(p.id, "science")
        else:
            p.research_overflow += sci
        if p.civic:
            p.civic_progress += cul
            cost = self.rules.civics[p.civic]["cost"]
            if p.civic_progress >= cost:
                p.civics.add(p.civic)
                p.civic_overflow = p.civic_progress - cost
                p.civic = None
                p.civic_progress = 0.0
        else:
            p.civic_overflow += cul

    def _process_city(self, p, city):
        ys = self.city_yields(city)
        city.food += ys["food"] - 2 * city.pop
        need = growth_threshold(city.pop)
        if city.food >= need:
            city.pop += 1
            city.food -= need
        elif city.food < 0:
            city.pop = max(1, city.pop - 1)
            city.food = 0.0
        city.production += ys["production"]
        if city.queue:
            item = city.queue[0]
            cost = self.item_cost(item)
            stalled = "unit" in item and item["unit"] == "settler" and city.pop < 2
            if not stalled and city.production >= cost:
                if self._complete_item(p, city, item):
                    city.production -= cost
                    city.queue.pop(0)
        city.border_culture += 1 + ys["culture"] * 0.5
        need_b = 15 + 8 * max(0, len(city.owned_tiles) - 7)
        if city.border_culture >= need_b:
            city.border_culture -= need_b
            self._expand_borders(city)
        city.hp = min(200, city.hp + 10)
        return ys

    def _complete_item(self, p, city, item):
        if "unit" in item:
            u = self._place_new_unit(item["unit"], p.id, city.pos)
            if u is None:
                return False
            if item["unit"] == "settler":
                city.pop -= 1
            return True
        if "building" in item:
            city.buildings.append(item["building"])
            return True
        pos = tuple(item["pos"])
        if pos in self.district_sites(city, item["district"]):
            t = self.map.get(pos)
            t.district = item["district"]
            t.improvement = None
            t.feature = None
            city.districts[item["district"]] = pos
        return True

    def _place_new_unit(self, utype, owner, pos):
        spec = self.rules.units[utype]
        want_sea = spec.get("domain") == "sea"
        for cand in [tuple(pos)] + hexgrid.neighbors(pos):
            t = self.map.get(cand)
            if t is None or not self.rules.is_passable(t):
                continue
            if self.rules.is_water(t) != want_sea:
                continue
            occupied = False
            for o in self.units_at(cand):
                if o.owner != owner or self.rules.units[o.type]["class"] == spec["class"]:
                    occupied = True
                    break
            c = self.city_at(cand)
            if c and c.owner != owner:
                occupied = True
            if not occupied:
                return self._spawn_unit(utype, owner, cand)
        return None

    def _expand_borders(self, city):
        best = None
        for pos in city.owned_tiles:
            for t in self.map.neighbors(pos):
                if t.owner_city is not None:
                    continue
                if hexgrid.distance(t.pos, city.pos) > 3:
                    continue
                tys = self.rules.tile_yields(t)
                val = sum(tys.values()) + (2 if t.resource else 0)
                key = (val, t.pos)
                if best is None or key > best[0]:
                    best = (key, t)
        if best:
            t = best[1]
            t.owner_city = city.id
            city.owned_tiles.append(t.pos)

    # ----------------------------------------------------------- win handling

    def _capture_city(self, city, new_owner):
        old = city.owner
        city.owner = new_owner
        city.pop = max(1, city.pop - 1)
        city.hp = 100
        city.queue = []
        if "walls" in city.buildings:
            city.buildings.remove("walls")
        for o in self.units_at(city.pos):
            if o.owner == old:
                o.owner = new_owner
        self._check_elimination(old)
        self._check_domination()

    def _on_unit_lost(self, pid):
        self._check_elimination(pid)
        self._check_domination()

    def _check_elimination(self, pid):
        p = self.players[pid]
        if not p.alive:
            return
        if self.player_cities(pid):
            return
        if any(u.type == "settler" for u in self.player_units(pid)):
            return
        p.alive = False
        for u in list(self.player_units(pid)):
            self._remove_unit(u.id)

    def _check_domination(self):
        alive = [p for p in self.players if p.alive and not p.is_minor]
        if len(alive) == 1:
            self._set_winner(alive[0].id, "domination")
            return
        capitals = [c for c in self.cities.values() if c.is_capital]
        if len(capitals) >= 2:
            owners = {c.owner for c in capitals}
            if len(owners) == 1:
                self._set_winner(owners.pop(), "domination")

    def _set_winner(self, pid, vtype):
        if self.winner is None:
            self.winner = pid
            self.victory_type = vtype

    # ---------------------------------------------------------- serialization

    def to_dict(self):
        s = self.rng.getstate()
        return {
            "seed": self.seed, "max_turns": self.max_turns, "turn": self.turn,
            "current": self.current, "winner": self.winner,
            "victory_type": self.victory_type, "next_id": self.next_id,
            "rng_state": [s[0], list(s[1]), s[2]],
            "at_war": sorted(sorted(pair) for pair in self.at_war),
            "map": self.map.to_dict(),
            "players": [p.to_dict() for p in self.players],
            "units": [u.to_dict() for u in self.units.values()],
            "cities": [c.to_dict() for c in self.cities.values()],
        }

    @classmethod
    def from_dict(cls, d, ruleset=None):
        g = cls(_skip_setup=True, seed=d["seed"], max_turns=d["max_turns"],
                ruleset=ruleset)
        g.turn = d["turn"]
        g.current = d["current"]
        g.winner = d["winner"]
        g.victory_type = d["victory_type"]
        g.next_id = d["next_id"]
        rs = d["rng_state"]
        g.rng.setstate((rs[0], tuple(rs[1]), rs[2]))
        g.at_war = {frozenset(pair) for pair in d["at_war"]}
        g.map = WorldMap.from_dict(d["map"])
        g.players = [Player.from_dict(pd) for pd in d["players"]]
        for ud in d["units"]:
            u = Unit.from_dict(ud)
            g.units[u.id] = u
            g._occ.setdefault(u.pos, []).append(u.id)
        for cd in d["cities"]:
            c = City.from_dict(cd)
            g.cities[c.id] = c
            g._city_by_pos[c.pos] = c.id
        return g

    def save(self, path):
        with open(path, "w", encoding="utf-8") as f:
            json.dump(self.to_dict(), f)

    @classmethod
    def load(cls, path, ruleset=None):
        with open(path, encoding="utf-8") as f:
            return cls.from_dict(json.load(f), ruleset=ruleset)
