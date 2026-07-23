import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


MODULE_PATH = Path(__file__).with_name("spectator_supervisor.py")
SPEC = importlib.util.spec_from_file_location("spectator_supervisor", MODULE_PATH)
assert SPEC and SPEC.loader
supervisor = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(supervisor)


class SessionSettingsTests(unittest.TestCase):
    def test_preserves_live_map_and_player_settings(self):
        state = {
            "players": [
                {"is_minor": False},
                {"is_minor": False},
                {"is_minor": True},
                {"is_minor": True, "is_barbarian": True},
            ],
            "map": {"width": 44, "height": 26, "script": "continents"},
            "game_speed": "online",
            "max_turns": 250,
        }
        defaults = {
            "players": 4,
            "width": 60,
            "height": 38,
            "city_states": 6,
            "turns": 500,
            "map": "pangaea",
            "speed": "standard",
        }
        self.assertEqual(
            supervisor.session_settings(state, defaults),
            {"players": 2, "width": 44, "height": 26, "city_states": 1,
             "turns": 250, "map": "continents", "speed": "online"},
        )

    def test_empty_state_uses_defaults(self):
        defaults = {
            "players": 6,
            "width": 74,
            "height": 46,
            "city_states": 9,
            "turns": 500,
            "map": "pangaea",
            "speed": "standard",
        }
        self.assertEqual(supervisor.session_settings({}, defaults), defaults)

    def test_result_standings_preserves_winner_and_excludes_non_major_players(self):
        state = {
            "winner": 2,
            "players": [
                {
                    "id": 0,
                    "civ": "Rome",
                    "score": 300,
                    "cities": 5,
                    "faith": 90,
                    "military": 240,
                },
                {
                    "id": 2,
                    "civ": "Egypt",
                    "score": 250,
                    "cities": 4,
                    "faith": 800,
                    "military": 120,
                },
                {"id": 4, "civ": "Geneva", "score": 999, "is_minor": True},
                {"id": 5, "civ": "Barbarians", "score": 999, "is_barbarian": True},
            ],
        }

        standings = supervisor.result_standings(state)

        self.assertEqual(
            standings,
            "Rome (score 300, cities 5, faith 90, military 240); "
            "winner Egypt (score 250, cities 4, faith 800, military 120)",
        )


class SourceSnapshotTests(unittest.TestCase):
    def test_snapshot_tracks_runtime_inputs_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            source = root / "src" / "lib.rs"
            source.write_text("pub fn value() -> u8 { 1 }\n", encoding="utf-8")
            readme = root / "README.md"
            readme.write_text("first\n", encoding="utf-8")
            with patch.object(supervisor, "ROOT", root):
                original = supervisor.source_snapshot()
                readme.write_text("second\n", encoding="utf-8")
                self.assertEqual(supervisor.source_snapshot(), original)
                source.write_text("pub fn value() -> u8 { 2 }\n", encoding="utf-8")
                self.assertNotEqual(supervisor.source_snapshot(), original)

    def test_runtime_dirtiness_is_scoped_to_compiled_inputs(self):
        clean = SimpleNamespace(returncode=0, stdout="")
        with patch.object(supervisor, "command", return_value=clean) as command:
            self.assertFalse(supervisor.runtime_inputs_dirty())
        command.assert_called_once_with(
            "git", "status", "--porcelain", "--", *supervisor.RUNTIME_INPUTS
        )

        changed = SimpleNamespace(returncode=0, stdout=" M src/game.rs\n")
        with patch.object(supervisor, "command", return_value=changed):
            self.assertTrue(supervisor.runtime_inputs_dirty())

    def test_changed_source_discards_obsolete_build_before_promoting(self):
        builds = []

        def fake_command(*args, **_kwargs):
            if args[:3] == ("cargo", "build", "--release"):
                builds.append(args)
            return SimpleNamespace(returncode=0, stdout="")

        with (
            patch.object(supervisor, "source_snapshot", side_effect=["old", "new", "new", "new"]),
            patch.object(supervisor, "command", side_effect=fake_command),
            patch.object(supervisor, "promote_binary") as promote,
            patch.object(supervisor, "write_runtime_metadata") as metadata,
        ):
            self.assertTrue(supervisor.build_latest())
        self.assertEqual(len(builds), 2)
        self.assertEqual(builds[0], ("cargo", "build", "--release", "--bin", "civvis"))
        promote.assert_called_once_with()
        metadata.assert_called_once_with("new")

    def test_failed_latest_build_never_promotes_stale_binary(self):
        failed = SimpleNamespace(returncode=1, stdout="compile error")
        with (
            patch.object(supervisor, "source_snapshot", return_value="current"),
            patch.object(supervisor, "command", return_value=failed),
            patch.object(supervisor, "promote_binary") as promote,
        ):
            self.assertFalse(supervisor.build_latest())
        promote.assert_not_called()

    def test_matching_runtime_skips_redundant_cargo_build(self):
        with (
            patch.object(supervisor, "source_snapshot", return_value="current"),
            patch.object(supervisor, "runtime_matches", return_value=True),
            patch.object(supervisor, "refresh_runtime_metadata") as refresh,
            patch.object(supervisor, "command") as command,
            patch.object(supervisor, "promote_binary") as promote,
        ):
            self.assertTrue(supervisor.build_latest())
        refresh.assert_called_once_with("current")
        command.assert_not_called()
        promote.assert_not_called()

    def test_exact_runtime_refreshes_stale_git_identity_without_rebuilding(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runtime = root / "civvis"
            runtime.write_bytes(b"verified binary")
            metadata_path = root / "build.json"
            metadata_path.write_text(
                json.dumps(
                    {
                        "revision": "local",
                        "dirty": True,
                        "source_snapshot": "same-source",
                        "binary_sha256": "stale",
                        "built_at": "original-build-time",
                    }
                ),
                encoding="utf-8",
            )
            revision = SimpleNamespace(returncode=0, stdout="published\n")
            with (
                patch.object(supervisor, "RUNTIME_BINARY", runtime),
                patch.object(supervisor, "RUNTIME_METADATA", metadata_path),
                patch.object(supervisor, "command", return_value=revision),
                patch.object(supervisor, "runtime_inputs_dirty", return_value=False),
            ):
                supervisor.refresh_runtime_metadata("same-source")

            refreshed = json.loads(metadata_path.read_text(encoding="utf-8"))
            self.assertEqual(refreshed["revision"], "published")
            self.assertFalse(refreshed["dirty"])
            self.assertEqual(refreshed["source_snapshot"], "same-source")
            self.assertEqual(refreshed["built_at"], "original-build-time")
            self.assertEqual(
                refreshed["binary_sha256"],
                supervisor.hashlib.sha256(runtime.read_bytes()).hexdigest(),
            )

    def test_matching_source_rejects_a_tampered_promoted_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runtime = root / "civvis"
            runtime.write_bytes(b"unexpected bytes")
            metadata_path = root / "build.json"
            metadata_path.write_text(
                json.dumps(
                    {
                        "source_snapshot": "same-source",
                        "binary_sha256": supervisor.hashlib.sha256(
                            b"expected bytes"
                        ).hexdigest(),
                    }
                ),
                encoding="utf-8",
            )
            with (
                patch.object(supervisor, "RUNTIME_BINARY", runtime),
                patch.object(supervisor, "RUNTIME_METADATA", metadata_path),
            ):
                self.assertFalse(supervisor.runtime_matches("same-source"))

    def test_single_update_attempt_returns_control_after_a_failed_build(self):
        with (
            patch.object(supervisor, "sync_current_branch") as sync,
            patch.object(supervisor, "build_latest", return_value=False) as build,
        ):
            self.assertFalse(supervisor.prepare_latest_once())
        sync.assert_called_once_with()
        build.assert_called_once_with(max_attempts=1)

    def test_runtime_replacement_distinguishes_in_process_restart_from_deployment(self):
        self.assertFalse(supervisor.runtime_replacement_pending("current", "current"))
        self.assertTrue(supervisor.runtime_replacement_pending("previous", "current"))
        self.assertTrue(supervisor.runtime_replacement_pending(None, "current"))
        self.assertFalse(supervisor.runtime_replacement_pending("previous", None))

    def test_boundary_uses_verified_runtime_instead_of_retrying_broken_source(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = Path(directory) / "civvis"
            runtime.touch()
            with (
                patch.object(supervisor, "RUNTIME_BINARY", runtime),
                patch.object(supervisor, "prepare_latest_once", return_value=False) as once,
                patch.object(supervisor, "prepare_latest") as retry,
            ):
                self.assertFalse(supervisor.prepare_boundary_runtime(15.0))
        once.assert_called_once_with()
        retry.assert_not_called()

    def test_boundary_waits_for_a_build_when_no_verified_runtime_exists(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = Path(directory) / "missing-civvis"
            with (
                patch.object(supervisor, "RUNTIME_BINARY", runtime),
                patch.object(supervisor, "prepare_latest_once", return_value=False),
                patch.object(supervisor, "prepare_latest") as retry,
            ):
                self.assertTrue(supervisor.prepare_boundary_runtime(7.5))
        retry.assert_called_once_with(7.5)

    def test_live_refresh_requires_both_fresh_code_and_a_safe_checkpoint(self):
        checkpoint = Path("/tmp/civvis-live-refresh.json")
        with (
            patch.object(supervisor, "prepare_latest_once", return_value=False),
            patch.object(supervisor, "capture_checkpoint") as capture,
        ):
            self.assertFalse(supervisor.prepare_live_refresh(8766, checkpoint))
        capture.assert_not_called()

        with (
            patch.object(supervisor, "prepare_latest_once", return_value=True),
            patch.object(supervisor, "capture_checkpoint", return_value=False),
        ):
            self.assertFalse(supervisor.prepare_live_refresh(8766, checkpoint))

        with (
            patch.object(supervisor, "prepare_latest_once", return_value=True),
            patch.object(supervisor, "capture_checkpoint", return_value=True) as capture,
        ):
            self.assertTrue(supervisor.prepare_live_refresh(8766, checkpoint))
        capture.assert_called_once_with(8766, checkpoint)

    def test_active_prebuild_skips_current_runtime_and_retries_changed_source(self):
        with (
            patch.object(supervisor, "source_snapshot", return_value="current"),
            patch.object(supervisor, "runtime_matches", return_value=True),
            patch.object(supervisor, "prepare_latest_once") as prepare,
        ):
            self.assertTrue(supervisor.prebuild_latest_once())
        prepare.assert_not_called()

        with (
            patch.object(supervisor, "source_snapshot", return_value="changed"),
            patch.object(supervisor, "runtime_matches", return_value=False),
            patch.object(supervisor, "prepare_latest_once", return_value=True) as prepare,
        ):
            self.assertTrue(supervisor.prebuild_latest_once())
        prepare.assert_called_once_with()


class RecoveryTests(unittest.TestCase):
    def test_successor_detection_closes_the_cooldown_restart_race(self):
        finished = {"server_instance": 7, "seed": 11, "winner": 2}
        self.assertFalse(supervisor.successor_started(None, 7, 11))
        self.assertFalse(supervisor.successor_started(finished, 7, 11))
        self.assertTrue(
            supervisor.successor_started({**finished, "winner": None}, 7, 11)
        )
        self.assertTrue(
            supervisor.successor_started({**finished, "seed": 12}, 7, 11)
        )

    def test_successor_grace_observes_the_server_owned_restart(self):
        finished = {"server_instance": 7, "seed": 11, "winner": 2}
        successor = {"server_instance": 7, "seed": 12, "winner": None}
        with patch.object(
            supervisor, "read_state", side_effect=[finished, successor]
        ):
            self.assertEqual(
                supervisor.wait_for_successor(8766, 7, 11, timeout=0.2),
                successor,
            )

    def test_active_compute_has_no_default_wall_clock_kill(self):
        self.assertFalse(
            supervisor.unavailable_recovery_due(
                True, 3_600.0, True, 60.0, 0.0
            )
        )
        self.assertTrue(
            supervisor.unavailable_recovery_due(
                True, 61.0, False, 60.0, 0.0
            )
        )
        self.assertTrue(
            supervisor.unavailable_recovery_due(
                True, 601.0, True, 60.0, 600.0
            )
        )
        self.assertTrue(
            supervisor.unavailable_recovery_due(
                False, 0.0, False, 60.0, 0.0
            )
        )

    def test_late_game_checkpoints_allow_slow_serialization(self):
        self.assertEqual(supervisor.capture_checkpoint.__defaults__, (30.0,))

    def test_progress_marker_tracks_player_steps_within_a_turn(self):
        first = {"seed": 7, "turn": 12, "current": 1, "winner": None}
        stepped = {**first, "current": 2}
        self.assertNotEqual(
            supervisor.progress_marker(first), supervisor.progress_marker(stepped)
        )

    def test_resume_detection_allows_progress_after_checkpoint_readiness(self):
        marker = (9, 22, 3, None)
        self.assertTrue(
            supervisor.resumed_checkpoint(
                {"seed": 9, "turn": 24, "current": 1, "winner": None}, marker
            )
        )
        self.assertFalse(
            supervisor.resumed_checkpoint(
                {"seed": 10, "turn": 1, "current": 0, "winner": None}, marker
            )
        )

    def test_stall_recovery_respects_an_intentional_browser_pause(self):
        self.assertTrue(supervisor.should_nudge({}, stalled_for=31, timeout=30))
        self.assertFalse(
            supervisor.should_nudge(
                {"spectator_paused": True}, stalled_for=300, timeout=30
            )
        )

    def test_server_command_can_resume_an_atomic_checkpoint(self):
        settings = {
            "players": 4,
            "width": 60,
            "height": 38,
            "city_states": 6,
            "turns": 500,
            "map": "pangaea",
            "speed": "standard",
        }
        checkpoint = Path("/tmp/civvis-checkpoint.json")
        command = supervisor.server_command(8766, settings, False, checkpoint)
        self.assertEqual(command[command.index("--resume") + 1], str(checkpoint))
        self.assertIn("--supervised", command)
        self.assertIn("--no-open", command)

    def test_checkpoint_write_is_atomic_and_finished_saves_are_not_resumed(self):
        class Response:
            def __init__(self, payload):
                self.payload = payload

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                return False

            def read(self):
                return self.payload

        active = {"seed": 9, "turn": 22, "current": 3, "winner": None}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "save.json"
            with patch.object(
                supervisor,
                "urlopen",
                return_value=Response(json.dumps(active).encode()),
            ):
                self.assertTrue(supervisor.capture_checkpoint(8766, path))
            self.assertEqual(json.loads(path.read_text()), active)
            self.assertFalse(path.with_suffix(".json.new").exists())
            self.assertEqual(supervisor.checkpoint_marker(path), (9, 22, 3, None))

            path.write_text(json.dumps({**active, "winner": 1}), encoding="utf-8")
            self.assertIsNone(supervisor.checkpoint_marker(path))

    def test_cold_supervisor_start_resumes_an_active_checkpoint(self):
        args = SimpleNamespace(
            port=8766,
            players=4,
            width=60,
            height=38,
            city_states=6,
            turns=500,
            map="pangaea",
            speed="standard",
            cooldown=10.0,
            poll=0.5,
            build_retry=15.0,
            source_check_interval=30.0,
            unresponsive_timeout=20.0,
            stall_timeout=30.0,
            checkpoint_interval=5.0,
            max_resume_attempts=2,
            no_open=True,
            adopt_pid=None,
        )
        state = {"seed": 9, "turn": 22, "current": 3, "winner": None}
        process = SimpleNamespace(pid=321)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runtime = root / "civvis"
            runtime.touch()
            checkpoint = root / "save.json"
            checkpoint.write_text(json.dumps(state), encoding="utf-8")
            with (
                patch.object(supervisor, "parse_args", return_value=args),
                patch.object(supervisor, "RUNTIME_BINARY", runtime),
                patch.object(supervisor, "checkpoint_path", return_value=checkpoint),
                patch.object(supervisor, "start_server", return_value=process) as start,
                patch.object(supervisor, "wait_for_server", return_value=state),
                patch.object(supervisor, "read_state", side_effect=KeyboardInterrupt),
                patch.object(supervisor, "stop_server") as stop,
            ):
                self.assertEqual(supervisor.main(), 0)
        start.assert_called_once_with(
            8766,
            {
                "players": 4,
                "width": 60,
                "height": 38,
                "city_states": 6,
                "turns": 500,
                "map": "pangaea",
                "speed": "standard",
            },
            False,
            checkpoint,
        )
        self.assertGreaterEqual(stop.call_count, 2)

    def test_adopted_stale_runtime_refreshes_and_resumes_without_waiting_for_a_win(self):
        args = SimpleNamespace(
            port=8766,
            players=4,
            width=60,
            height=38,
            city_states=6,
            turns=500,
            map="pangaea",
            speed="standard",
            cooldown=0.0,
            poll=0.01,
            build_retry=0.01,
            source_check_interval=30.0,
            unresponsive_timeout=20.0,
            busy_timeout=0.0,
            stall_timeout=30.0,
            checkpoint_interval=5.0,
            max_resume_attempts=2,
            no_open=True,
            adopt_pid=321,
        )
        active = {"seed": 9, "turn": 42, "current": 2, "winner": None}
        replacement = SimpleNamespace(pid=654)
        events = []

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runtime = root / "civvis"
            runtime.touch()
            checkpoint = root / "save.json"

            def prepare(_port, path):
                path.write_text(json.dumps(active), encoding="utf-8")
                events.append(("prepare", None))
                return True

            def stop(process, adopted_pid):
                events.append(("stop", getattr(process, "pid", adopted_pid)))

            def start(*args, **_kwargs):
                events.append(("start", args[3]))
                return replacement

            with (
                patch.object(supervisor, "parse_args", return_value=args),
                patch.object(supervisor, "RUNTIME_BINARY", runtime),
                patch.object(supervisor, "checkpoint_path", return_value=checkpoint),
                patch.object(supervisor, "process_alive", return_value=True),
                patch.object(supervisor, "runtime_matches", return_value=False),
                patch.object(supervisor, "source_snapshot", return_value="fresh"),
                patch.object(supervisor, "prepare_live_refresh", side_effect=prepare),
                patch.object(supervisor, "capture_checkpoint", return_value=False),
                patch.object(supervisor, "read_state", side_effect=[active, active, KeyboardInterrupt]),
                patch.object(supervisor, "start_server", side_effect=start),
                patch.object(supervisor, "wait_for_server", return_value=active),
                patch.object(supervisor, "stop_server", side_effect=stop),
            ):
                self.assertEqual(supervisor.main(), 0)

        self.assertLess(events.index(("prepare", None)), events.index(("stop", 321)))
        self.assertIn(("start", checkpoint), events)

    def test_finished_server_starts_successor_without_waiting_for_a_build(self):
        args = SimpleNamespace(
            port=8766,
            players=4,
            width=60,
            height=38,
            city_states=6,
            turns=500,
            map="pangaea",
            speed="standard",
            cooldown=0.0,
            poll=0.01,
            build_retry=0.01,
            source_check_interval=30.0,
            unresponsive_timeout=20.0,
            busy_timeout=600.0,
            stall_timeout=30.0,
            checkpoint_interval=5.0,
            max_resume_attempts=2,
            no_open=True,
            adopt_pid=None,
        )
        active = {"seed": 9, "turn": 22, "current": 3, "winner": None}
        finished = {
            **active,
            "turn": 70,
            "winner": 1,
            "victory_type": "science",
            "players": [],
        }
        successor = {"seed": 10, "turn": 1, "current": 0, "winner": None}
        first_process = SimpleNamespace(pid=321)
        second_process = SimpleNamespace(pid=654)
        events = []
        starts = []

        def start(*_args, **_kwargs):
            process = first_process if not starts else second_process
            starts.append(process)
            events.append(("start", process.pid))
            return process

        def wait(_port, process):
            events.append(("wait", process.pid))
            return active if process is first_process else successor

        def stop(process, _adopted_pid):
            events.append(("stop", getattr(process, "pid", None)))

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runtime = root / "civvis"
            runtime.touch()
            checkpoint = root / "save.json"
            with (
                patch.object(supervisor, "parse_args", return_value=args),
                patch.object(supervisor, "RUNTIME_BINARY", runtime),
                patch.object(supervisor, "checkpoint_path", return_value=checkpoint),
                patch.object(supervisor, "runtime_matches", return_value=False),
                patch.object(supervisor, "start_server", side_effect=start),
                patch.object(supervisor, "wait_for_server", side_effect=wait),
                patch.object(
                    supervisor,
                    "read_state",
                    side_effect=[finished, KeyboardInterrupt],
                ),
                patch.object(supervisor, "stop_server", side_effect=stop),
            ):
                self.assertEqual(supervisor.main(), 0)

        retired = events.index(("stop", 321))
        launched = events.index(("start", 654))
        self.assertLess(retired, launched)
        self.assertNotIn(("prepare", None), events)


if __name__ == "__main__":
    unittest.main()
