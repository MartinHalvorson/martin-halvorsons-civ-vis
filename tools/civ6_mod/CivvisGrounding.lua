-- CIVVIS grounding instrumentation for Civilization VI.
--
-- Two jobs, both in service of comparing this game's behaviour to CIVVIS':
--
--   1. Run the game unattended. The engine's autoplay manager can play every
--      player with the game's own AI for a fixed number of turns. That turns a
--      250-turn game into something a harness can run and re-run, which is the
--      only way to collect trajectories in quantity.
--
--   2. Record what happened, per turn, per player, as one machine-readable
--      line per turn. The game's own CSV logs already carry scores and stats;
--      what they do not carry is the shape of each empire -- which districts
--      exist where, what each city is building, what each player is
--      researching. Those are the decisions a strategy makes, so they are what
--      a strategy comparison needs.
--
-- Every line this writes is prefixed CIVVISJSON so a tail can find it in
-- Lua.log among the engine's own chatter, and carries the run tag from the
-- config so lines from an earlier run in the same log are separable.
--
-- The mod changes no rules. It reads state and drives autoplay, nothing else.

-- The run's settings arrive as a `CivvisGroundingConfig` table that
-- tools/civ6_run.py prepends to this file at install time.
--
-- It used to be a separate file pulled in with `include`. That silently did
-- nothing: listing a file under <Files> in the .modinfo makes it part of the
-- mod, but only an ImportFiles action puts it somewhere `include` can find, so
-- the call failed, every setting fell back to its default, and a run
-- configured for 250 turns of autoplay ran zero. Prepending removes the
-- lookup, and with it the failure.
local PREFIX = "CIVVISJSON ";
-- Read as a plain global, not through `_G`. The UI Lua sandbox does not expose
-- `_G`, so `rawget(_G, ...)` raises at load -- which kills the whole script
-- before it can report anything, and looks exactly like a mod that was never
-- applied. Reading an undefined global is simply nil in Lua, so this is safe.
local cfg = CivvisGroundingConfig or {};
local started = false;

-- ---------------------------------------------------------------- JSON

-- Civ 6's Lua has no JSON encoder. This one covers exactly the value types
-- this mod produces: nil, boolean, number, string, array-like and map-like
-- tables. It is deliberately small rather than general.
local function esc(s)
	s = tostring(s);
	s = s:gsub("\\", "\\\\"):gsub('"', '\\"');
	s = s:gsub("\n", "\\n"):gsub("\r", "\\r"):gsub("\t", "\\t");
	return s;
end

local encode;
encode = function(v)
	local t = type(v);
	if v == nil then
		return "null";
	elseif t == "boolean" then
		return v and "true" or "false";
	elseif t == "number" then
		-- Integers print without a trailing ".0"; the consumer parses both,
		-- but integral output keeps the lines readable and small.
		if v == math.floor(v) and v == v and v ~= math.huge and v ~= -math.huge then
			return string.format("%d", v);
		end
		return string.format("%.6g", v);
	elseif t == "string" then
		return '"' .. esc(v) .. '"';
	elseif t == "table" then
		local n = 0;
		for _ in pairs(v) do n = n + 1; end
		local isArray = (#v == n);
		local parts = {};
		if isArray then
			for i = 1, #v do parts[#parts + 1] = encode(v[i]); end
			return "[" .. table.concat(parts, ",") .. "]";
		end
		-- Stable key order: a diff between two runs should show real changes,
		-- not Lua's hash order.
		local keys = {};
		for k in pairs(v) do keys[#keys + 1] = tostring(k); end
		table.sort(keys);
		for _, k in ipairs(keys) do
			parts[#parts + 1] = '"' .. esc(k) .. '":' .. encode(v[k]);
		end
		return "{" .. table.concat(parts, ",") .. "}";
	end
	return '"<' .. t .. '>"';
end

-- Where a line can actually land is build-dependent: this macOS build writes no
-- Lua.log at all, so `print` alone would make a working mod look dead. Every
-- available sink gets the line, and the redundancy is deliberate -- a channel
-- that silently goes nowhere is the failure this mod exists to avoid.
local function emit(kind, payload)
	payload.kind = kind;
	payload.run = cfg.RunTag or "unset";
	local line = PREFIX .. encode(payload);
	pcall(function() print(line); end);
	if Automation ~= nil and Automation.Log ~= nil then
		pcall(function() Automation.Log(line); end);
	end
	if UI ~= nil and UI.DataError ~= nil then
		pcall(function() UI.DataError(line); end);
	end
end

-- Anything read out of the game API is wrapped: a nil method on some ruleset
-- or a player in an odd state must not take the whole dump down, because a
-- dump that stops halfway is worse than one with a missing field -- it looks
-- like the game ended.
local function try(fn, fallback)
	local ok, result = pcall(fn);
	if ok then return result; end
	return fallback;
end

-- ------------------------------------------------------- capability probe

-- Which of the engine's globals this context actually has decides what the
-- harness can do. Guessing produced a wrong conclusion once already (a tuner
-- port that was never open because the options file was the wrong one), so
-- this is reported as data rather than assumed.
local function probeCapabilities()
	-- Named getters rather than a `_G` walk: the sandbox has no `_G`, and an
	-- undefined global simply reads as nil, so each of these is safe.
	local probes = {
		{ "AutoplayManager", function() return AutoplayManager; end },
		{ "Automation", function() return Automation; end },
		{ "Game", function() return Game; end },
		{ "GameConfiguration", function() return GameConfiguration; end },
		{ "Players", function() return Players; end },
		{ "PlayerManager", function() return PlayerManager; end },
		{ "Map", function() return Map; end },
		{ "UI", function() return UI; end },
		{ "UnitManager", function() return UnitManager; end },
		{ "CityManager", function() return CityManager; end },
		{ "Network", function() return Network; end },
		{ "GameInfo", function() return GameInfo; end },
		{ "Benchmark", function() return Benchmark; end },
	};
	local present, absent = {}, {};
	for _, probe in ipairs(probes) do
		local ok, value = pcall(probe[2]);
		if ok and value ~= nil then
			present[#present + 1] = probe[1];
		else
			absent[#absent + 1] = probe[1];
		end
	end
	local autoplayApi = {};
	if AutoplayManager ~= nil then
		for _, fn in ipairs({ "SetActive", "SetTurns", "SetObserveAsPlayer",
		                      "SetReturnAsPlayer", "IsActive" }) do
			autoplayApi[fn] = (type(AutoplayManager[fn]) == "function");
		end
	end
	emit("capabilities", {
		present = present,
		absent = absent,
		autoplay = autoplayApi,
		config = {
			AutoplayTurns = cfg.AutoplayTurns,
			ObserveAsPlayer = cfg.ObserveAsPlayer,
			DumpState = cfg.DumpState,
		},
	});
end

-- ------------------------------------------------------------- state dump

local function cityRecord(city)
	local rec = {
		name = try(function() return Locale.Lookup(city:GetName()); end, "?"),
		x = try(function() return city:GetX(); end, -1),
		y = try(function() return city:GetY(); end, -1),
		pop = try(function() return city:GetPopulation(); end, 0),
	};
	rec.food = try(function() return city:GetYield(YieldTypes.FOOD); end, 0);
	rec.production = try(function() return city:GetYield(YieldTypes.PRODUCTION); end, 0);
	rec.science = try(function() return city:GetYield(YieldTypes.SCIENCE); end, 0);
	rec.culture = try(function() return city:GetYield(YieldTypes.CULTURE); end, 0);
	rec.gold = try(function() return city:GetYield(YieldTypes.GOLD); end, 0);
	rec.faith = try(function() return city:GetYield(YieldTypes.FAITH); end, 0);

	-- What the city is building is the visible end of a build-order policy,
	-- which is most of what a CIVVIS strategy genome encodes.
	rec.building = try(function()
		local queue = city:GetBuildQueue();
		if queue == nil then return "" end
		local hash = queue:GetCurrentProductionTypeHash();
		if hash == nil or hash == 0 then return "" end
		local info = GameInfo.Units[hash] or GameInfo.Buildings[hash]
			or GameInfo.Districts[hash] or GameInfo.Projects[hash];
		return info and info.Type or "";
	end, "");

	rec.districts = try(function()
		local out = {};
		local districts = city:GetDistricts();
		if districts == nil then return out end
		for _, district in districts:Members() do
			local info = GameInfo.Districts[district:GetType()];
			if info ~= nil and district:IsComplete() then
				out[#out + 1] = info.DistrictType;
			end
		end
		table.sort(out);
		return out;
	end, {});

	return rec;
end

local function playerRecord(playerId)
	local player = Players[playerId];
	if player == nil then return nil; end

	local rec = {
		id = playerId,
		civ = try(function()
			return PlayerConfigurations[playerId]:GetCivilizationTypeName();
		end, "?"),
		leader = try(function()
			return PlayerConfigurations[playerId]:GetLeaderTypeName();
		end, "?"),
		alive = try(function() return player:IsAlive(); end, false),
		major = try(function() return player:IsMajor(); end, false),
	};
	if not rec.alive then return rec; end

	rec.score = try(function() return player:GetScore(); end, 0);
	rec.gold = try(function() return player:GetTreasury():GetGoldBalance(); end, 0);
	rec.faith = try(function() return player:GetReligion():GetFaithBalance(); end, 0);
	rec.techs = try(function() return player:GetTechs():GetNumTechsResearched(); end, 0);
	rec.civics = try(function() return player:GetCulture():GetNumCivicsCompleted(); end, 0);
	rec.era = try(function() return player:GetEras():GetEra(); end, -1);

	rec.researching = try(function()
		local techs = player:GetTechs();
		local id = techs:GetResearchingTech();
		if id == nil or id < 0 then return "" end
		local info = GameInfo.Technologies[id];
		return info and info.TechnologyType or "";
	end, "");
	rec.civic_progress = try(function()
		local culture = player:GetCulture();
		local id = culture:GetProgressingCivic();
		if id == nil or id < 0 then return "" end
		local info = GameInfo.Civics[id];
		return info and info.CivicType or "";
	end, "");

	rec.units = try(function()
		local n = 0;
		for _, _ in player:GetUnits():Members() do n = n + 1; end
		return n;
	end, 0);

	rec.cities = try(function()
		local out = {};
		for _, city in player:GetCities():Members() do
			out[#out + 1] = cityRecord(city);
		end
		return out;
	end, {});

	return rec;
end

local function dumpTurn()
	if cfg.DumpState == false then return; end
	local turn = try(function() return Game.GetCurrentGameTurn(); end, -1);
	local players = {};
	for _, playerId in ipairs(PlayerManager.GetAliveMajorIDs()) do
		local rec = playerRecord(playerId);
		if rec ~= nil then players[#players + 1] = rec; end
	end
	emit("turn", { turn = turn, players = players });
end

-- ---------------------------------------------------------------- autoplay

local function startAutoplay()
	local turns = tonumber(cfg.AutoplayTurns) or 0;
	if turns <= 0 then
		emit("autoplay", { started = false, reason = "AutoplayTurns is 0" });
		return;
	end
	if AutoplayManager == nil then
		emit("autoplay", {
			started = false,
			reason = "AutoplayManager is not available in this context",
		});
		return;
	end
	local ok, err = pcall(function()
		AutoplayManager.SetTurns(turns);
		AutoplayManager.SetReturnAsPlayer(cfg.ReturnAsPlayer or 0);
		AutoplayManager.SetObserveAsPlayer(cfg.ObserveAsPlayer or -1);
		AutoplayManager.SetActive(true);
	end);
	emit("autoplay", { started = ok, turns = turns, error = ok and nil or tostring(err) });
end

-- ------------------------------------------------------------------ events

-- One-time setup, driven off whichever hook fires first.
--
-- `LoadGameViewStateDone` is the event the shipped UI uses for "the game is
-- ready", but a mod context is created too late to see it: the event has
-- already been raised by the time this script exists, so a handler added here
-- never runs. Anything gated behind it -- the capability probe, and starting
-- autoplay -- silently never happens, while the turn dump keeps working and
-- makes the mod look healthy.
--
-- So setup runs from the first hook of either kind, and is idempotent.
local function ensureStarted()
	if started then return; end
	started = true;
	pcall(probeCapabilities);
	pcall(startAutoplay);
end

local function onTurnBegin()
	local ok, err = pcall(dumpTurn);
	if not ok then
		emit("error", { where = "dumpTurn", error = tostring(err) });
	end
	ensureStarted();
end

local function onLoadDone()
	ensureStarted();
	onTurnBegin();
end

local function onAutoPlayEnd()
	emit("autoplay_end", { turn = try(function() return Game.GetCurrentGameTurn(); end, -1) });
	onTurnBegin();
end

function Initialize()
	emit("loaded", { version = 1 });
	Events.LoadGameViewStateDone.Add(onLoadDone);
	Events.TurnBegin.Add(onTurnBegin);
	if LuaEvents ~= nil and LuaEvents.AutoPlayEnd ~= nil then
		LuaEvents.AutoPlayEnd.Add(onAutoPlayEnd);
	end
end

Initialize();
