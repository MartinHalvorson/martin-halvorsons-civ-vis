"""Random map generation: continents, climate bands, features, resources, spawns."""
from . import hexgrid
from .world import WorldMap


def generate(rules, width, height, num_players, rng):
    wm = WorldMap(width, height)
    tiles = wm.tiles

    # --- landmass via random frontier growth
    land_target = int(0.42 * width * height)
    land = set()
    for _ in range(max(2, num_players // 2 + 1)):
        col = rng.randint(width // 5, width - 1 - width // 5)
        row = rng.randint(height // 5, height - 1 - height // 5)
        land.add(hexgrid.offset_to_axial(col, row))
    frontier = sorted(land)
    for _ in range(40 * width * height):
        if len(land) >= land_target:
            break
        if not frontier:
            frontier = [sorted(land)[rng.randrange(len(land))]]
        cur = frontier[rng.randrange(len(frontier))]
        nbs = [n for n in hexgrid.neighbors(cur) if n in tiles and n not in land]
        if not nbs:
            frontier.remove(cur)
            continue
        nxt = nbs[rng.randrange(len(nbs))]
        land.add(nxt)
        frontier.append(nxt)
        if rng.random() < 0.25:
            frontier.remove(cur)

    land_list = sorted(land)

    # --- climate bands
    def latitude(pos):
        _, row = hexgrid.axial_to_offset(*pos)
        return abs(2.0 * row / max(1, height - 1) - 1.0)

    for pos in land_list:
        v = latitude(pos) + rng.uniform(-0.15, 0.15)
        t = tiles[pos]
        if v > 0.85:
            t.terrain = "snow"
        elif v > 0.62:
            t.terrain = "tundra"
        elif v < 0.30:
            t.terrain = rng.choices(["desert", "plains", "grassland"], [0.25, 0.40, 0.35])[0]
        else:
            t.terrain = rng.choices(["grassland", "plains", "desert"], [0.50, 0.42, 0.08])[0]

    # --- mountain chains, hills
    for _ in range(max(2, len(land_list) // 40)):
        cur = land_list[rng.randrange(len(land_list))]
        for _ in range(rng.randint(2, 5)):
            tiles[cur].terrain = "mountain"
            nbs = [n for n in hexgrid.neighbors(cur) if n in land]
            if not nbs:
                break
            cur = nbs[rng.randrange(len(nbs))]
    for pos in land_list:
        if tiles[pos].terrain != "mountain" and rng.random() < 0.16:
            tiles[pos].hills = True

    # --- coast
    for pos, t in tiles.items():
        if t.terrain == "ocean" and any(n in land for n in hexgrid.neighbors(pos)):
            t.terrain = "coast"

    # --- features
    for pos in land_list:
        t = tiles[pos]
        if t.terrain == "mountain":
            continue
        r = rng.random()
        if t.terrain in ("grassland", "plains"):
            if latitude(pos) < 0.25 and r < 0.28:
                t.feature = "jungle"
            elif r < 0.20:
                t.feature = "forest"
            elif t.terrain == "grassland" and r > 0.97:
                t.feature = "marsh"
        elif t.terrain == "tundra" and r < 0.22:
            t.feature = "forest"
        elif t.terrain == "desert" and r < 0.05:
            t.feature = "oasis"

    # --- resources
    for pos in sorted(tiles):
        t = tiles[pos]
        if t.terrain == "mountain" or t.feature in ("oasis", "marsh"):
            continue
        if rng.random() < 0.13:
            valid = []
            for name, s in rules.resources.items():
                if s.get("feature"):
                    if t.feature in s["feature"]:
                        valid.append(name)
                elif t.terrain in s.get("terrain", []):
                    valid.append(name)
            if valid:
                t.resource = valid[rng.randrange(len(valid))]

    # --- spawns on the largest connected passable landmass
    passable = {p for p in land if tiles[p].terrain != "mountain"}
    largest = _largest_component(passable)
    cands = sorted(p for p in largest
                   if tiles[p].terrain in ("grassland", "plains") and not tiles[p].feature)
    if len(cands) < num_players:
        cands = sorted(largest)
    spawns = [cands[rng.randrange(len(cands))]]
    while len(spawns) < num_players:
        pool = [c for c in cands if c not in spawns]
        best = max(pool, key=lambda c: (min(hexgrid.distance(c, s) for s in spawns), c))
        spawns.append(best)
    for s in spawns:
        tiles[s].feature = None
        tiles[s].resource = None
    return wm, spawns


def _largest_component(cells):
    seen = set()
    best = set()
    for start in sorted(cells):
        if start in seen:
            continue
        comp = {start}
        stack = [start]
        while stack:
            cur = stack.pop()
            for n in hexgrid.neighbors(cur):
                if n in cells and n not in comp:
                    comp.add(n)
                    stack.append(n)
        seen |= comp
        if len(comp) > len(best):
            best = comp
    return best
