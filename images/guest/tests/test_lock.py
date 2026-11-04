import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from guest.lock import load_lock, validate_lock, locked_file, input_identity, VERSIONS
from unittest.mock import patch
from guest.tests.fixtures import lock


class LockTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.value = lock(self.root)

    def test_source_lock(self):
        validate_lock(self.value, production=False)
        path = self.root / "input.json"
        path.write_text(json.dumps(self.value))
        self.assertEqual(load_lock(path), self.value)

    def test_source_identity_mismatch(self):
        self.value["orka"]["identity"] = "0" * 40
        with self.assertRaisesRegex(ValueError, "source"):
            validate_lock(self.value, production=False)

    def test_superseded_source_pin_is_rejected(self):
        previous = "aa3ca00524347ea4124f6751e16790a019d21d73"
        self.value["sourceCommit"] = previous
        self.value["orka"]["identity"] = previous
        with self.assertRaisesRegex(ValueError, "source identity"):
            validate_lock(self.value, production=False)

    def test_release_missing_833(self):
        with self.assertRaisesRegex(ValueError, "release"):
            validate_lock(self.value, production=True)
        self.value["orka"].update(mode="release", identity="v0.3.0")
        with self.assertRaisesRegex(ValueError, "833|profile|release"):
            validate_lock(self.value, production=True)

    def test_general_worker_and_loader_required(self):
        for name in ["general-worker", "session-loader", "teams", "telegram"]:
            value = copy.deepcopy(self.value)
            value["components"] = [c for c in value["components"] if c["name"] != name]
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "component"):
                validate_lock(value, production=False)

    def test_production_rejects_fixture(self):
        self.value["orka"].update(mode="release", identity="fixture")
        with self.assertRaisesRegex(ValueError, "no reviewed release profile"):
            validate_lock(self.value, production=True)

    def test_reviewed_profile_binds_all_input_identities(self):
        self.value["orka"].update(mode="release", identity="reviewed-test-release")
        self.value["kmxProfileSha256"] = "1" * 64
        profile = {"amd64": input_identity(self.value)}
        with patch.dict(VERSIONS["releaseProfiles"], {"reviewed-test-release": profile}):
            validate_lock(self.value, production=True)
            changed = copy.deepcopy(self.value)
            changed["components"][1]["manifestDigest"] = "sha256:" + "2" * 64
            with self.assertRaisesRegex(ValueError, "profile"):
                validate_lock(changed, production=True)
            changed = copy.deepcopy(self.value)
            changed["tools"][0]["sha256"] = "2" * 64
            with self.assertRaisesRegex(ValueError, "profile"):
                validate_lock(changed, production=True)

    def test_no_moving_refs(self):
        for field in ["manifestDigest", "configDigest"]:
            value = copy.deepcopy(self.value)
            value["components"][0][field] = "latest"
            with self.subTest(field=field), self.assertRaises(ValueError):
                validate_lock(value, production=False)
        self.value["tools"][0]["version"] = "latest"
        with self.assertRaises(ValueError):
            validate_lock(self.value, production=False)

    def test_unknown_fields_duplicate_names_and_keys(self):
        self.value["token"] = "not-a-real-secret"
        with self.assertRaises(ValueError):
            validate_lock(self.value, production=False)
        del self.value["token"]
        self.value["components"].append(self.value["components"][0])
        with self.assertRaises(ValueError):
            validate_lock(self.value, production=False)
        path = self.root / "duplicate.json"
        path.write_text('{"architecture":"amd64","architecture":"arm64"}')
        with self.assertRaisesRegex(ValueError, "duplicate"):
            load_lock(path)

    def test_input_paths_and_symlinks(self):
        for name in ["../outside", "/etc/passwd", "a/../chart.tgz"]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                locked_file(self.root, name)
        (self.root / "link").symlink_to(self.root / "chart.tgz")
        with self.assertRaises(ValueError):
            locked_file(self.root, "link")
        self.assertEqual((self.root / "chart.tgz").read_bytes(), b"synthetic-chart")

    def test_repository_has_an_explicit_registry_host(self):
        for name in ["orka/controller", "docker/library/busybox"]:
            self.value["components"][0]["repository"] = name
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "repository"):
                validate_lock(self.value, production=False)
        for name in ["ghcr.io/orka/controller", "localhost/controller", "registry:5000/controller"]:
            self.value["components"][0]["repository"] = name
            validate_lock(self.value, production=False)

    def test_deep_json_reports_a_validation_error(self):
        path = self.root / "nested.json"
        path.write_text("{}")
        with patch("guest.lock.json.loads", side_effect=RecursionError("injected decoder depth failure")):
            with self.assertRaisesRegex(ValueError, "JSON"):
                load_lock(path)

    def test_architecture_and_node_tag(self):
        self.value["architecture"] = "x86_64"
        with self.assertRaises(ValueError):
            validate_lock(self.value, production=False)
        self.value["architecture"] = "amd64"
        self.value["nodeLocalRef"] = "kindest/node:latest"
        with self.assertRaises(ValueError):
            validate_lock(self.value, production=False)

    def test_documented_release_cli_rejects_source_semantically(self):
        path = self.root / "source.json"
        path.write_text(json.dumps(self.value))
        proc = subprocess.run([sys.executable, "images/guest/lock.py", "validate", "--lock", str(path), "--mode", "release"], capture_output=True, text=True)
        self.assertEqual(proc.returncode, 2)
        self.assertIn("Orka release including #833", proc.stderr)
        self.assertNotIn("invalid choice", proc.stderr)

    def test_cli_reports_error_without_traceback(self):
        path = self.root / "invalid.json"
        path.write_text("{}")
        proc = subprocess.run([sys.executable, "images/guest/lock.py", "validate", "--lock", str(path), "--mode", "source"], capture_output=True, text=True)
        self.assertNotEqual(proc.returncode, 0)
        self.assertNotIn("Traceback", proc.stderr)
        self.assertEqual(proc.stdout, "")
