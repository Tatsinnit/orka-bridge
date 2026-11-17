import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from guest.lock import load_lock, validate_lock, locked_file, input_identity, validate_source_offline, VERSIONS, MODEL_IMAGES
from unittest.mock import patch
from guest.tests.fixtures import lock


class LockTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.value = lock(self.root)

    def test_selected_source_pin_binds_the_restricted_offline_profile(self):
        self.assertEqual(VERSIONS['orkaSource'], '3cb1f0df36705af9ff81cb8b4e8a0f8d36ee6eea')
        self.assertEqual(VERSIONS.get('sourceOfflineProfile'), {
            'profile': 'orka-3cb1f0df-native/v1',
            'profileSha256': '20647cab1e653394fe7466608a08dd816f05d1a44bc8a8598fa478f7e20fc8f5',
            'kmxSourceCommit': '503b3ffcbdcc0a966e3b6de6235a8a98077b989e',
            'chartSha256': '11cc020b923dd5bc810a6f3b97f236a6e3123ce8ad80791d912c0af760abcca8',
            'artifactsSha256': '6f620c97411465b1f1559b57f6d279313894dcf0006ae25f8121e35afbb66080',
        })
        self.assertEqual(set(MODEL_IMAGES), {VERSIONS['sourceOfflineProfile']['kmxSourceCommit']})
        self.assertEqual(VERSIONS['releaseProfiles'], {})

    def test_source_offline_accepts_merged_kmx_and_rejects_superseded_revision(self):
        selected = VERSIONS['sourceOfflineProfile']
        self.value['kmxProfileSha256'] = selected['profileSha256']
        self.value['orka']['chartSha256'] = selected['chartSha256']
        self.value['tools'].extend([
            {'name': 'kmx'},
            {'name': 'orka-images', 'sha256': selected['artifactsSha256']},
            {'name': 'kmx-offline-manifest'},
        ])
        manifest = {'profile': 'orka-3cb1f0df-native/v1',
                    'kmx': {'revision': '503b3ffcbdcc0a966e3b6de6235a8a98077b989e'}}
        try:
            validate_source_offline(self.value, manifest)
        except ValueError as error:
            self.fail(f'merged kmx source revision must be accepted: {error}')
        manifest['kmx']['revision'] = 'e58f763cdda79ac41c81a1a695559c7aabfe4dc0'
        with self.assertRaisesRegex(ValueError, 'source offline profile or kmx revision differs'):
            validate_source_offline(self.value, manifest)

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
        previous = "59703d6eac1760aa2dc5fed4dee9dcb74c6e5186"
        self.value["sourceCommit"] = previous
        self.value["orka"]["identity"] = previous
        with self.assertRaisesRegex(ValueError, "source identity"):
            validate_lock(self.value, production=False)

    def preview_lock(self):
        selected = VERSIONS['sourceOfflineProfile']
        self.value['kmxProfileSha256'] = selected['profileSha256']
        self.value['orka']['chartSha256'] = selected['chartSha256']
        self.value['tools'].extend([
            dict(name=name, version='fixture-1', path=name, sha256=digest)
            for name, digest in [('kmx', '1' * 64),
                                 ('orka-images', selected['artifactsSha256']),
                                 ('kmx-offline-manifest', '2' * 64)]
        ])
        return {self.value['sourceCommit']: {'amd64': input_identity(self.value)}}

    def test_publisher_preview_requires_explicit_exact_identity(self):
        previews = self.preview_lock()
        validate_lock(self.value, production=True, source_previews=previews)
        with self.assertRaisesRegex(ValueError, 'release'):
            validate_lock(self.value, production=True)
        for mutate in [lambda v: v['tools'][0].update(sha256='3' * 64),
                       lambda v: v.update(architecture='arm64', nodeLocalRef=v['nodeLocalRef'].replace('amd64', 'arm64'))]:
            value = copy.deepcopy(self.value)
            mutate(value)
            with self.assertRaisesRegex(ValueError, 'preview'):
                validate_lock(value, production=True, source_previews=previews)
        path = self.root / 'preview.json'
        path.write_text(json.dumps(self.value))
        result = subprocess.run([sys.executable, str(Path(__file__).parents[1] / 'lock.py'), 'validate',
                                 '--mode', 'release', '--lock', str(path)],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn('release', result.stderr)

    def test_even_approved_preview_requires_offline_closure_and_pins(self):
        approvals = self.preview_lock()
        for mutation in ('missing', 'wrong-profile'):
            value = copy.deepcopy(self.value)
            if mutation == 'missing':
                value['tools'] = [t for t in value['tools'] if t['name'] != 'kmx-offline-manifest']
            else:
                value['kmxProfileSha256'] = 'f' * 64
            previews = {value['sourceCommit']: {'amd64': input_identity(value)}}
            with self.subTest(mutation=mutation), self.assertRaisesRegex(ValueError, 'offline'):
                validate_lock(value, production=True, source_previews=previews)
        value = copy.deepcopy(self.value)
        value['orka'].update(mode='release', identity='unreviewed')
        with self.assertRaisesRegex(ValueError, 'release profile'):
            validate_lock(value, production=True, source_previews=approvals)

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
