"""Minimal ASCII renderer for debugging headless games."""
from . import hexgrid

TERRAIN_CHARS = {"grassland": ".", "plains": ",", "desert": "d", "tundra": ":",
                 "snow": "*", "coast": "-", "ocean": "~", "mountain": "^"}
FEATURE_CHARS = {"forest": "f", "jungle": "j", "marsh": "m", "oasis": "o"}


def ascii_map(game, pid=None):
    explored = game.players[pid].explored if pid is not None else None
    rows = []
    for row in range(game.map.height):
        cells = []
        for col in range(game.map.width):
            pos = hexgrid.offset_to_axial(col, row)
            t = game.map.get(pos)
            if t is None:
                continue
            if explored is not None and pos not in explored:
                cells.append("??")
                continue
            city = game.city_at(pos)
            units = game.units_at(pos)
            if city is not None:
                cells.append("#" + str(city.owner))
            elif units:
                u = units[0]
                cells.append(u.type[0].upper() + str(u.owner))
            else:
                ch = TERRAIN_CHARS.get(t.terrain, "?")
                mod = "+" if t.hills else FEATURE_CHARS.get(t.feature, " ")
                if t.district:
                    mod = "@"
                cells.append(ch + mod)
        rows.append((" " if row % 2 else "") + "".join(cells))
    return "\n".join(rows)
