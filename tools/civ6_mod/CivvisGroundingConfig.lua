-- Configuration for the CIVVIS grounding mod.
--
-- This file is the inbound channel. Civilization VI's Lua cannot read an
-- arbitrary file at runtime, but it does `include` its own mod files at load,
-- so the harness rewrites this one before launching a game and the mod picks
-- the values up on the next start. That is enough for per-run settings; it is
-- not a per-turn command channel and is not meant to be one.
--
-- tools/civ6_run.py rewrites this file. Edits here are overwritten.

CivvisGroundingConfig = {
	-- Turns of autoplay to run once the game is loaded. 0 disables autoplay
	-- and leaves the game under manual control.
	AutoplayTurns = 0,

	-- Which player the camera observes while autoplay runs. -1 observes none,
	-- which is the cheapest to render.
	ObserveAsPlayer = -1,

	-- Player the game hands back to when autoplay ends.
	ReturnAsPlayer = 0,

	-- Write the per-turn state record.
	DumpState = true,

	-- Tag written into every log line, so a run's lines can be separated from
	-- an earlier run's in the same Lua.log.
	RunTag = "unset",
}
