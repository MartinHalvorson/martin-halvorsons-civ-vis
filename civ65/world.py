"""Tiles and the world map."""
from . import hexgrid

YIELD_KEYS = ("food", "production", "gold", "science", "culture", "faith")


def zero_yields():
    return {k: 0.0 for k in YIELD_KEYS}


def add_yields(a, b):
    for k, v in b.items():
        a[k] = a.get(k, 0.0) + v
    return a


class Tile:
    __slots__ = ("pos", "terrain", "feature", "hills", "resource",
                 "improvement", "district", "owner_city")

    def __init__(self, pos, terrain="ocean", feature=None, hills=False,
                 resource=None, improvement=None, district=None, owner_city=None):
        self.pos = tuple(pos)
        self.terrain = terrain
        self.feature = feature
        self.hills = hills
        self.resource = resource
        self.improvement = improvement
        self.district = district
        self.owner_city = owner_city

    def to_dict(self):
        return {"pos": list(self.pos), "terrain": self.terrain, "feature": self.feature,
                "hills": self.hills, "resource": self.resource,
                "improvement": self.improvement, "district": self.district,
                "owner_city": self.owner_city}

    @classmethod
    def from_dict(cls, d):
        return cls(pos=tuple(d["pos"]), terrain=d["terrain"], feature=d["feature"],
                   hills=d["hills"], resource=d["resource"], improvement=d["improvement"],
                   district=d["district"], owner_city=d["owner_city"])


class WorldMap:
    def __init__(self, width, height, fill=True):
        self.width = width
        self.height = height
        self.tiles = {}
        if fill:
            for row in range(height):
                for col in range(width):
                    pos = hexgrid.offset_to_axial(col, row)
                    self.tiles[pos] = Tile(pos)

    def get(self, pos):
        return self.tiles.get(tuple(pos))

    def neighbors(self, pos):
        return [self.tiles[n] for n in hexgrid.neighbors(pos) if n in self.tiles]

    def to_dict(self):
        return {"width": self.width, "height": self.height,
                "tiles": [t.to_dict() for t in self.tiles.values()]}

    @classmethod
    def from_dict(cls, d):
        wm = cls(d["width"], d["height"], fill=False)
        for td in d["tiles"]:
            t = Tile.from_dict(td)
            wm.tiles[t.pos] = t
        return wm
