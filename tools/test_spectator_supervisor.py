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
            "map": {"width": 44, "height": 26},
        }
        defaults = {
            "players": 4,
            "width": 60,
            "height": 38,
            "city_states": 6,
            "turns": 500,
        }
        self.assertEqual(
            supervisor.session_settings(state, defaults),
            {"players": 2, "width": 44, "height": 26, "city_states": 1, "turns": 500},
        )

    def test_empty_state_uses_defaults(self):
        defaults = {
            "players": 6,
            "width": 74,
            "height": 46,
            "city_states": 9,
            "turns": 500,
        }
        self.assertEqual(supervisor.session_settings({}, defaults), defaults)


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
            patch.object(supervisor, "command") as command,
            patch.object(supervisor, "promote_binary") as promote,
        ):
            self.assertTrue(supervisor.build_latest())
        command.assert_not_called()
        promote.assert_not_called()

    def test_single_update_attempt_returns_control_after_a_failed_build(self):
        with (
            patch.object(supervisor, "sync_current_branch") as sync,
            patch.object(supervisor, "build_latest", return_value=False) as build,
        ):
            self.assertFalse(supervisor.prepare_latest_once())
        sync.assert_called_once_with()
        build.assert_called_once_with(max_attempts=1)


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

    def test_progress_marker_tracks_player_steps_within_a_turn(self):
        first = {"seed": 7, "turn": 12, "current": 1, "winner": None}
        stepped = {**first, "current": 2}
        self.assertNotEqual(
            supervisor.progress_marker(first), supervisor.progress_marker(stepped)
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
        }
        checkpoint = Path("/tmp/civvis-checkpoint.json")
        command = supervisor.server_command(8766, settings, False, checkpoint)
        self.assertEqual(command[command.index("--resume") + 1], str(checkpoint))
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
            cooldown=10.0,
            poll=0.5,
            build_retry=15.0,
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
            },
            False,
            checkpoint,
        )
        self.assertGreaterEqual(stop.call_count, 2)


if __name__ == "__main__":
    unittest.main()
