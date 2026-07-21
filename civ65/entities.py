"""Game entities: units, cities, players."""
from dataclasses import dataclass, field


@dataclass
class Unit:
    id: int
    type: str
    owner: int
    pos: tuple
    hp: int = 100
    moves_left: float = 0.0
    charges: int = 0

    def to_dict(self):
        return {"id": self.id, "type": self.type, "owner": self.owner,
                "pos": list(self.pos), "hp": self.hp,
                "moves_left": self.moves_left, "charges": self.charges}

    @classmethod
    def from_dict(cls, d):
        d = dict(d)
        d["pos"] = tuple(d["pos"])
        return cls(**d)


@dataclass
class City:
    id: int
    name: str
    owner: int
    pos: tuple
    pop: int = 1
    food: float = 0.0
    production: float = 0.0
    border_culture: float = 0.0
    hp: int = 200
    buildings: list = field(default_factory=list)
    districts: dict = field(default_factory=dict)   # name -> pos tuple
    owned_tiles: list = field(default_factory=list)  # pos tuples
    queue: list = field(default_factory=list)        # production item dicts
    original_owner: int = 0
    is_capital: bool = False

    def to_dict(self):
        return {"id": self.id, "name": self.name, "owner": self.owner,
                "pos": list(self.pos), "pop": self.pop, "food": self.food,
                "production": self.production, "border_culture": self.border_culture,
                "hp": self.hp, "buildings": list(self.buildings),
                "districts": {k: list(v) for k, v in self.districts.items()},
                "owned_tiles": [list(p) for p in self.owned_tiles],
                "queue": self.queue, "original_owner": self.original_owner,
                "is_capital": self.is_capital}

    @classmethod
    def from_dict(cls, d):
        d = dict(d)
        d["pos"] = tuple(d["pos"])
        d["districts"] = {k: tuple(v) for k, v in d["districts"].items()}
        d["owned_tiles"] = [tuple(p) for p in d["owned_tiles"]]
        return cls(**d)


@dataclass
class Player:
    id: int
    civ: str
    techs: set = field(default_factory=set)
    research: str = None
    research_progress: float = 0.0
    research_overflow: float = 0.0
    civics: set = field(default_factory=set)
    civic: str = None
    civic_progress: float = 0.0
    civic_overflow: float = 0.0
    gold: float = 0.0
    faith: float = 0.0
    explored: set = field(default_factory=set)
    alive: bool = True
    is_minor: bool = False  # city-state

    def to_dict(self):
        return {"id": self.id, "civ": self.civ, "is_minor": self.is_minor,
                "techs": sorted(self.techs),
                "research": self.research, "research_progress": self.research_progress,
                "research_overflow": self.research_overflow,
                "civics": sorted(self.civics), "civic": self.civic,
                "civic_progress": self.civic_progress, "civic_overflow": self.civic_overflow,
                "gold": self.gold, "faith": self.faith,
                "explored": sorted([list(p) for p in self.explored]),
                "alive": self.alive}

    @classmethod
    def from_dict(cls, d):
        d = dict(d)
        d.setdefault("is_minor", False)
        d["techs"] = set(d["techs"])
        d["civics"] = set(d["civics"])
        d["explored"] = {tuple(p) for p in d["explored"]}
        return cls(**d)
