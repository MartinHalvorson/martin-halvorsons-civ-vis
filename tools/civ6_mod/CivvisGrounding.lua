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
	-- Counted by asking about each entry rather than by a summary getter.
	-- `GetNumTechsResearched` exists on some rulesets and not this one; when it
	-- is missing the guarded read yields its fallback, so every player reports
	-- zero techs on every turn -- a flat line that looks like real data and
	-- would silently invalidate any tech-pace comparison.
	rec.techs = try(function()
		local techs, n = player:GetTechs(), 0;
		for row in GameInfo.Technologies() do
			if techs:HasTech(row.Index) then n = n + 1; end
		end
		return n;
	end, -1);
	rec.civics = try(function()
		local culture, n = player:GetCulture(), 0;
		for row in GameInfo.Civics() do
			if culture:HasCivic(row.Index) then n = n + 1; end
		end
		return n;
	end, -1);
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

-- ------------------------------------------------------- the strategy agent

-- Play a CIVVIS league genome in the real game.
--
-- The genome is ~40 weights, but most of them are tactical -- how a force
-- groups, when it withdraws -- and cannot transfer, because here the shipped
-- AI moves the units. What transfers is the economic policy, and that is also
-- what separates the top strategies from each other: Maverick2 targets 4.4
-- cities, WildCard10 targets 9.7, off the same opening.
--
-- So this drives production, which is where those weights become visible
-- decisions: the scripted opening, then settlers until `city_target`, then
-- builders and military to their per-city targets, then districts in the
-- genome's priority order. Everything else stays with the game's own AI, so a
-- comparison measures the policy rather than a half-finished reimplementation
-- of Civilization.
--
-- Every decision is logged with the reason that produced it. A genome that
-- silently fails to apply looks exactly like a genome that does not matter.

local genome = CivvisGenome;
local bookPos = 0;
local decisions = 0;

local function infoHash(kind, typeName)
	local row = kind[typeName];
	return row and row.Hash or nil;
end

local function canProduce(city, hash)
	if hash == nil then return false; end
	local ok, result = pcall(function()
		return city:GetBuildQueue():CanProduce(hash, true);
	end);
	return ok and result == true;
end

-- Issue the build. Districts additionally need a plot, and picking one is its
-- own problem, so they are attempted and allowed to fail rather than faked.
local function requestBuild(city, typeName)
	local row = GameInfo.Types[typeName];
	if row == nil then return false; end
	local params = {};
	local kind = row.Kind;
	local hash = row.Hash;
	if kind == "KIND_UNIT" then
		params[CityOperationTypes.PARAM_UNIT_TYPE] = hash;
	elseif kind == "KIND_BUILDING" then
		params[CityOperationTypes.PARAM_BUILDING_TYPE] = hash;
	elseif kind == "KIND_DISTRICT" then
		params[CityOperationTypes.PARAM_DISTRICT_TYPE] = hash;
	else
		return false;
	end
	local ok = pcall(function()
		CityManager.RequestOperation(city, CityOperationTypes.BUILD, params);
	end);
	return ok;
end

local function unitCounts(player)
	local counts = { settler = 0, builder = 0, military = 0, total = 0 };
	local ok = pcall(function()
		for _, unit in player:GetUnits():Members() do
			local row = GameInfo.Units[unit:GetUnitType()];
			local name = row and row.UnitType or "";
			counts.total = counts.total + 1;
			if name == "UNIT_SETTLER" then
				counts.settler = counts.settler + 1;
			elseif name == "UNIT_BUILDER" then
				counts.builder = counts.builder + 1;
			elseif row ~= nil and (row.Combat or 0) > 0 then
				counts.military = counts.military + 1;
			end
		end
	end);
	if not ok then return counts; end
	return counts;
end

-- Military units this ruleset lets the city build, cheapest-first fallbacks.
local MILITARY_LADDER = { "UNIT_SWORDSMAN", "UNIT_ARCHER", "UNIT_SPEARMAN",
                          "UNIT_SLINGER", "UNIT_WARRIOR" };

local DISTRICT_WEIGHTS = {
	{ "d_campus", "DISTRICT_CAMPUS" },
	{ "d_commercial", "DISTRICT_COMMERCIAL_HUB" },
	{ "d_holy", "DISTRICT_HOLY_SITE" },
	{ "d_theater", "DISTRICT_THEATER" },
};

local function chooseDistrict(city)
	-- Highest genome weight first; the genome's whole point is this ordering.
	local ranked = {};
	for _, pair in ipairs(DISTRICT_WEIGHTS) do
		ranked[#ranked + 1] = { weight = tonumber(genome[pair[1]]) or 0, type = pair[2] };
	end
	table.sort(ranked, function(a, b) return a.weight > b.weight; end);
	for _, entry in ipairs(ranked) do
		local row = GameInfo.Districts[entry.type];
		if row ~= nil and canProduce(city, row.Hash) then
			return entry.type, string.format("district w=%.2f", entry.weight);
		end
	end
	return nil, nil;
end

local function chooseItem(player, city, turn, counts, nCities)
	-- 1. The scripted opening, in the capital, exactly as CIVVIS plays it.
	-- The book advances when a build *completes*, never when a decision is
	-- merely reconsidered. Advancing per call consumed all four entries on
	-- turn 1 -- the agent is asked for a decision many times a turn -- so the
	-- scripted opening was skipped and the city built whatever rule 2 onward
	-- asked for. The opening is the most legible part of a genome; playing it
	-- out of order quietly invalidates the comparison.
	local isCapital = try(function() return city:IsCapital(); end, false);
	if isCapital and bookPos < 4 then
		local genes = { genome.open0, genome.open1, genome.open2, genome.open3 };
		-- Skip past "pass" genes (an index past the menu means "evaluate
		-- normally"), but stop on the first entry that is actually playable.
		while bookPos < 4 do
			local gene = tonumber(genes[bookPos + 1]);
			local name = nil;
			if gene ~= nil then
				local index = math.floor(math.max(0, gene));
				name = genome.OpeningMenu and genome.OpeningMenu[index + 1] or nil;
			end
			if name == nil then
				bookPos = bookPos + 1;  -- a pass gene: it is consumed, not played
			else
				local typeName = (name == "monument") and "BUILDING_MONUMENT"
					or ("UNIT_" .. string.upper(name));
				local row = GameInfo.Types[typeName];
				if row ~= nil and canProduce(city, row.Hash) then
					return typeName, "opening[" .. bookPos .. "]=" .. name;
				end
				-- Not buildable yet (no settle site, missing tech): leave the
				-- entry in place and let a later turn play it.
				break;
			end
		end
	end

	-- 2. Expansion, which is the lever that most separates these genomes.
	local target = tonumber(genome.city_target) or 4;
	local stopTurn = tonumber(genome.settler_stop_turn) or 150;
	local minPop = tonumber(genome.settler_min_pop) or 2;
	local pop = try(function() return city:GetPopulation(); end, 0);
	if (nCities + counts.settler) < target and turn < stopTurn and pop >= minPop then
		if canProduce(city, infoHash(GameInfo.Units, "UNIT_SETTLER")) then
			return "UNIT_SETTLER", string.format(
				"expand %d+%d<%.1f", nCities, counts.settler, target);
		end
	end

	-- 3. Standing builder and army targets.
	local builderTarget = (tonumber(genome.builder_per_city) or 0.5) * nCities;
	if counts.builder < builderTarget then
		if canProduce(city, infoHash(GameInfo.Units, "UNIT_BUILDER")) then
			return "UNIT_BUILDER", string.format(
				"builders %d<%.1f", counts.builder, builderTarget);
		end
	end
	local milTarget = (tonumber(genome.mil_per_city) or 1.0) * nCities;
	if counts.military < milTarget then
		for _, name in ipairs(MILITARY_LADDER) do
			if canProduce(city, infoHash(GameInfo.Units, name)) then
				return name, string.format("army %d<%.1f", counts.military, milTarget);
			end
		end
	end

	-- 4. Districts, in the genome's priority order.
	local district, why = chooseDistrict(city);
	if district ~= nil then return district, why; end

	return nil, nil;
end

local function driveProduction()
	if genome == nil then return; end
	local pid = tonumber(cfg.StrategyPlayer);
	if pid == nil or pid < 0 then return; end
	local player = Players[pid];
	if player == nil or not try(function() return player:IsAlive(); end, false) then return; end

	local turn = try(function() return Game.GetCurrentGameTurn(); end, 0);
	local counts = unitCounts(player);
	local cities = {};
	pcall(function()
		for _, city in player:GetCities():Members() do cities[#cities + 1] = city; end
	end);
	local nCities = #cities;

	for _, city in ipairs(cities) do
		-- Replace whatever the shipped AI queued, not merely fill an empty
		-- queue. Under autoplay the AI picks a build the instant one finishes,
		-- so a queue is never observed empty and a fill-only agent issues zero
		-- orders while looking perfectly healthy.
		--
		-- Civ 6 banks partial production per item, so switching away and back
		-- does not burn what was already invested; and the order is only sent
		-- when the genome actually disagrees, so a city already building the
		-- right thing is left alone.
		local current = try(function()
			local queue = city:GetBuildQueue();
			return queue and queue:GetCurrentProductionTypeHash() or 0;
		end, 0);
		local typeName, why = chooseItem(player, city, turn, counts, nCities);
		if typeName ~= nil then
			local wanted = GameInfo.Types[typeName];
			local wantedHash = wanted and wanted.Hash or nil;
			if wantedHash ~= nil and wantedHash ~= current then
				local applied = requestBuild(city, typeName);
				decisions = decisions + 1;
				emit("decision", {
					turn = turn,
					player = pid,
					city = try(function() return Locale.Lookup(city:GetName()); end, "?"),
					item = typeName,
					reason = why,
					applied = applied,
					replaced = try(function()
						local row = GameInfo.Types[current];
						return row and row.Type or "";
					end, ""),
					cities = nCities,
					settlers = counts.settler,
					builders = counts.builder,
					military = counts.military,
				});
			end
		end
	end
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
	local droveOk, droveErr = pcall(driveProduction);
	if not droveOk then
		emit("error", { where = "driveProduction", error = tostring(droveErr) });
	end
end

-- A city that just finished something is idle until someone fills the queue,
-- and waiting for the next turn boundary would waste a turn of production on
-- every completion. Choosing immediately is also what the genome describes.
local function onProductionCompleted(playerId)
	-- A completed build is what retires an opening-book entry.
	if playerId == nil or playerId == tonumber(cfg.StrategyPlayer) then
		if bookPos < 4 then bookPos = bookPos + 1; end
	end
	local ok, err = pcall(driveProduction);
	if not ok then
		emit("error", { where = "onProductionCompleted", error = tostring(err) });
	end
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
	emit("loaded", {
		version = 2,
		genome = genome and genome.Name or nil,
		strategy_player = cfg.StrategyPlayer,
	});
	Events.LoadGameViewStateDone.Add(onLoadDone);
	Events.TurnBegin.Add(onTurnBegin);
	if Events.CityProductionCompleted ~= nil then
		Events.CityProductionCompleted.Add(onProductionCompleted);
	end
	if LuaEvents ~= nil and LuaEvents.AutoPlayEnd ~= nil then
		LuaEvents.AutoPlayEnd.Add(onAutoPlayEnd);
	end
end

Initialize();
