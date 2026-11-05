"""Local source-input contract tests; no downloads, compilers or guests."""
import contextlib
import copy
import hashlib
import io
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import recipe

REFERENCE = "a" * 40
QEMU = "b" * 40
PATCHES = ("0001", "0005", "0009", "0015", "0019", "0024")
CHECKS = ("test-socket-close.py", "test-fd-setsize.py", "test-whpx-cpu-model.py")


def fixture():
    def entry(path, origin):
        data = path.encode()
        return {"path": path, "url": origin, "bytes": len(data),
                "sha256": hashlib.sha256(data).hexdigest()}
    ref = f"https://raw.githubusercontent.com/omacom/try-omarchy-windows/{REFERENCE}/runtime-build/"
    qemu = f"https://raw.githubusercontent.com/cmspam/winq-emu-qemu/{QEMU}/"
    return {"schemaVersion": "bridge-windows-sources/v1", "sourceDateEpoch": 1777269494,
            "reference": {"repository": "https://github.com/omacom/try-omarchy-windows", "commit": REFERENCE},
            "qemu": {"repository": "https://github.com/cmspam/winq-emu-qemu", "commit": QEMU, "baseCommit": "c" * 40},
            "patches": [entry(f"patches/qemu/{n}-fixture.patch", ref + f"patches/qemu/{n}-fixture.patch") for n in PATCHES],
            "checks": [entry(f"checks/{n}", ref + n) for n in CHECKS],
            "wraps": [entry("subprojects/example.wrap", qemu + "subprojects/example.wrap")],
            "submodules": [{"path": "roms/seabios", "repository": "https://gitlab.com/qemu-project/seabios.git", "commit": "d" * 40}]}


class Sources(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.lock = self.root / "sources.json"

    def load(self, value=None):
        self.lock.write_text(json.dumps(value if value is not None else fixture()), encoding="utf-8")
        return recipe.load_sources(self.lock)

    def stage(self, lock):
        inputs = self.root / "inputs"
        inputs.mkdir()
        for group in ("patches", "checks", "wraps"):
            for item in lock[group]:
                path = inputs / item["path"]
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(item["path"].encode())
        return inputs

    def test_valid_lock_and_input_bytes(self):
        lock = self.load()
        inputs = self.stage(lock)
        self.assertEqual(recipe.check_sources(lock, inputs), 10)

    def test_missing_unknown_and_bad_fields(self):
        mutations = [lambda d: d.pop("qemu"), lambda d: d.update(extra=1),
                     lambda d: d.update(sourceDateEpoch=True),
                     lambda d: d["qemu"].update(commit="main"),
                     lambda d: d["patches"][0].update(sha256="0" * 63),
                     lambda d: d["patches"][0].update(bytes=True)]
        for mutate in mutations:
            with self.subTest(mutate=mutate):
                data = fixture()
                mutate(data)
                with self.assertRaises(ValueError):
                    self.load(data)

    def test_patch_bytes_and_order(self):
        for change in (lambda d: d["patches"].reverse(), lambda d: d["patches"].pop(),
                       lambda d: d["checks"].pop(), lambda d: d["patches"].append(d["patches"][0])):
            data = fixture()
            change(data)
            with self.assertRaises(ValueError):
                self.load(data)

    def test_immutable_origins(self):
        for group in ("patches", "checks", "wraps"):
            data = fixture()
            data[group][0]["url"] = "https://example.com/latest"
            with self.assertRaises(ValueError):
                self.load(data)

    def test_windows_path_aliases(self):
        for name in ("../x", "/x", "C:/x", "a\\b", "a//b", "a/./b", "a/NUL", "a/COM1.txt", "a/b.", "a/b ", "a/b:c"):
            data = fixture()
            data["wraps"][0]["path"] = name
            data["wraps"][0]["url"] = f"https://raw.githubusercontent.com/cmspam/winq-emu-qemu/{QEMU}/{name}"
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "unsafe source path"):
                self.load(data)

    def test_duplicate_check_basename(self):
        data = fixture()
        duplicate = copy.deepcopy(data["checks"][0])
        duplicate["path"] = "checks/extra/test-socket-close.py"
        duplicate["url"] = duplicate["url"].replace("/test-socket-close.py", "/extra/test-socket-close.py")
        data["checks"].append(duplicate)
        with self.assertRaisesRegex(ValueError, "required source checks"):
            self.load(data)

    def test_duplicate_keys_and_paths(self):
        self.lock.write_text('{"schemaVersion":1,"schemaVersion":2}', encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "duplicate"):
            recipe.load_sources(self.lock)
        data = fixture()
        data["submodules"].append(copy.deepcopy(data["submodules"][0]))
        with self.assertRaises(ValueError):
            self.load(data)

    def test_corrupt_missing_and_nonregular_inputs(self):
        lock = self.load()
        inputs = self.stage(lock)
        path = inputs / lock["patches"][0]["path"]
        path.write_bytes(b"corrupt")
        with self.assertRaisesRegex(ValueError, "0001"):
            recipe.check_sources(lock, inputs)
        path.unlink()
        with self.assertRaises(ValueError):
            recipe.check_sources(lock, inputs)
        path.mkdir()
        with self.assertRaises(ValueError):
            recipe.check_sources(lock, inputs)

    def test_equal_length_corruption(self):
        lock = self.load()
        inputs = self.stage(lock)
        path = inputs / lock["wraps"][0]["path"]
        path.write_bytes(b"x" * path.stat().st_size)
        with self.assertRaisesRegex(ValueError, "sha256"):
            recipe.check_sources(lock, inputs)

    def test_symlink_component(self):
        lock = self.load()
        inputs = self.stage(lock)
        folder = inputs / "subprojects"
        file = folder / "example.wrap"
        target = self.root / "example.wrap"
        file.replace(target)
        folder.rmdir()
        try:
            folder.symlink_to(self.root, target_is_directory=True)
        except OSError:
            self.skipTest("host does not grant symlink creation")
        with self.assertRaises(ValueError):
            recipe.check_sources(lock, inputs)

    def test_cli_distinguishes_structure_from_bytes(self):
        self.load()
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(recipe.main(["check-sources", "--lock", str(self.lock)]), 0)
        result = json.loads(output.getvalue())
        self.assertFalse(result["publicationReady"])
        self.assertFalse(result["inputBytesVerified"])
        self.assertEqual(result["filesVerified"], 0)
        inputs = self.stage(self.load())
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            recipe.main(["check-sources", "--lock", str(self.lock), "--inputs", str(inputs)])
        self.assertTrue(json.loads(output.getvalue())["inputBytesVerified"])

    def test_cli_failure_has_no_success_json(self):
        self.load()
        with contextlib.redirect_stdout(io.StringIO()) as out, contextlib.redirect_stderr(io.StringIO()) as err:
            code = recipe.main(["check-sources", "--lock", str(self.lock), "--inputs", str(self.root / "absent")])
        self.assertEqual(code, 1)
        self.assertEqual(out.getvalue(), "")
        self.assertIn("source inputs", err.getvalue())

    def test_checked_in_lock_and_no_execution(self):
        with patch("subprocess.run", side_effect=AssertionError("no subprocess")), patch("subprocess.check_output", side_effect=AssertionError("no subprocess")):
            lock = recipe.load_sources(Path(__file__).with_name("sources.lock.json"))
        self.assertEqual(len(lock["patches"]), 6)
        self.assertEqual(len(lock["checks"]), 3)

    def test_subprocess_cli(self):
        self.load()
        result = subprocess.run([sys.executable, str(Path(recipe.__file__)), "check-sources", "--lock", str(self.lock)], capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(json.loads(result.stdout)["inputBytesVerified"])


class Workflow(unittest.TestCase):
    def test_read_only_path_filtered_source_validation(self):
        root = Path(__file__).resolve().parents[2]
        text = (root / ".github/workflows/windows-runtime.yml").read_text(encoding="utf-8")
        self.assertIn("pull_request:", text)
        self.assertIn("push:", text)
        self.assertEqual(text.count("- 'runtime/windows/**'"), 2)
        self.assertEqual(text.count("- '.github/workflows/windows-runtime.yml'"), 2)
        self.assertIn("contents: read", text)
        self.assertIn("persist-credentials: false", text)
        self.assertIn("github.event.pull_request.head.sha || github.sha", text)
        self.assertIn("GIT_CONFIG_KEY_0: core.autocrlf", text)
        self.assertIn("GIT_CONFIG_VALUE_0: 'false'", text)
        self.assertIn("cancel-in-progress: true", text)
        self.assertIn("windows-2025", text)
        self.assertIn("check-sources --lock runtime/windows/sources.lock.json", text)
        self.assertIn("python -m unittest discover -s runtime/windows", text)
        for forbidden in ("pull_request_target", "contents: write", "secrets.", "upload-artifact", "qemu-system", "pacman", "workflow_dispatch"):
            self.assertNotIn(forbidden, text)
        for action in re.findall(r"uses: ([^\s]+)", text):
            self.assertRegex(action, r"@[a-f0-9]{40}$")


if __name__ == "__main__":
    unittest.main()
