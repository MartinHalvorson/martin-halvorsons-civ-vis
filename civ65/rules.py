"""Ruleset: all game content loaded from JSON data files (moddable, Unciv-style)."""
import json
import os

from .world import add_yields, zero_yields


class Ruleset:
    CATEGORIES = ("terrains", "features", "resources", "improvements", "units",
                  "districts", "buildings", "techs", "civics")

    def __init__(self, data_dir=None):
        data_dir = data_dir or os.path.join(os.path.dirname(__file__), "data")
        for cat in self.CATEGORIES:
            with open(os.path.join(data_dir, cat + ".json"), encoding="utf-8") as f:
                setattr(self, cat, json.load(f))

    def tile_yields(self, tile):
        """Yields of a worked (non-district) tile."""
        ys = zero_yields()
        add_yields(ys, self.terrains[tile.terrain].get("yields", {}))
        if tile.hills:
            ys["production"] += 1
        if tile.feature:
            add_yields(ys, self.features[tile.feature].get("yields", {}))
        if tile.resource:
            add_yields(ys, self.resources[tile.resource].get("yields", {}))
        if tile.improvement:
            add_yields(ys, self.improvements[tile.improvement].get("yields", {}))
        return ys

    def is_water(self, tile):
        return self.terrains[tile.terrain].get("water", False)

    def is_passable(self, tile):
        return self.terrains[tile.terrain].get("passable", True)

    def move_cost(self, tile):
        c = self.terrains[tile.terrain].get("move_cost", 1)
        if tile.feature:
            c = max(c, self.features[tile.feature].get("move_cost", 1))
        if tile.hills:
            c = max(c, 2)
        return c
