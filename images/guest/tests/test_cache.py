import copy
import gzip
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import subprocess
import sys

from guest.cache import collect, inspect_archive
from guest.lock import load_lock
from guest.tests.fixtures import archive, encoded, lock, sha


class CacheTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.value = lock(self.root)

    def test_deterministic_exact_reference_closure(self):
        receipts = []
        for name in ["one", "two"]:
            out = self.root / name
            collect(self.value, out, source=self.root)
            receipt = json.loads((out / "collection.json").read_text())
            self.assertFalse(receipt["publicationReady"])
            self.assertEqual(receipt["architecture"], "amd64")
            for component in receipt["components"]:
                path = out / component["archive"]
                self.assertEqual(sha(path.read_bytes()), component["archiveSha256"])
                with tarfile.open(path) as tar:
                    index = json.load(tar.extractfile("index.json"))
                    descriptor = index["manifests"][0]
                    self.assertEqual(descriptor["annotations"]["org.opencontainers.image.ref.name"], component["reference"])
                    self.assertEqual(descriptor["digest"], component["manifestDigest"])
                    self.assertTrue(all(m.mtime == 0 for m in tar.getmembers()))
            receipts.append((out / "collection.json").read_bytes())
        self.assertEqual(*receipts)

    def test_normalized_lock_is_consumable(self):
        out = self.root / "out"
        collect(self.value, out, source=self.root)
        normalized = load_lock(out / "inputs.json")
        self.assertEqual(normalized["components"][0]["archive"], "components/kind-node.oci.tar")
        self.assertEqual(normalized["orka"]["chartPath"], "chart.tgz")
        receipt = json.loads((out / "collection.json").read_text())
        self.assertEqual(receipt["normalizedLockSha256"], sha((out / "inputs.json").read_bytes()))
        self.assertNotIn(str(self.root), (out / "collection.json").read_text())
        collect(normalized, self.root / "recollected", source=out)

    def test_non_object_and_malformed_oci_json(self):
        for data in [b"null", b"[]", b"true"]:
            c = archive(self.root, "badjson", extra={"index.json": data})
            with self.subTest(data=data), self.assertRaises(ValueError):
                inspect_archive(c, self.root, "amd64")

    def test_wrong_platform(self):
        c = archive(self.root, "wrong", architecture="arm64")
        with self.assertRaisesRegex(ValueError, "platform"):
            inspect_archive(c, self.root, "amd64")

    def test_archive_hash_mismatch(self):
        self.value["components"][0]["archiveSha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "hash|checksum"):
            collect(self.value, self.root / "out", source=self.root)
        self.assertFalse((self.root / "out").exists())

    def test_chart_or_tool_hash_mismatch(self):
        for path in ["chart.tgz", "kind"]:
            original = (self.root / path).read_bytes()
            (self.root / path).write_bytes(b"changed")
            with self.subTest(path=path), self.assertRaises(ValueError):
                collect(self.value, self.root / "out", source=self.root)
            self.assertFalse((self.root / "out").exists())
            (self.root / path).write_bytes(original)

    def test_unsafe_duplicate_and_extra_archive_members(self):
        for bad in ["../../escape", "/absolute", "unused.txt"]:
            c = archive(self.root, "bad", extra={bad: b"data"})
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                inspect_archive(c, self.root, "amd64")
        c = archive(self.root, "duplicate")
        path = self.root / c["archive"]
        with tarfile.open(path, "a") as tar:
            data = b"{}"
            info = tarfile.TarInfo("index.json"); info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
        c["archiveSha256"] = sha(path.read_bytes())
        with self.assertRaisesRegex(ValueError, "duplicate"):
            inspect_archive(c, self.root, "amd64")

    def test_truncated_gzip_is_a_validation_error(self):
        c = self.value["components"][0]
        compressed = gzip.compress((self.root / c["archive"]).read_bytes(), mtime=0)
        truncated = compressed[:len(compressed) // 2]
        (self.root / "truncated.tar.gz").write_bytes(truncated)
        c.update(archive="truncated.tar.gz", archiveSha256=sha(truncated))
        with self.assertRaisesRegex(ValueError, "invalid OCI archive"):
            inspect_archive(c, self.root, "amd64")
        path = self.root / "input.json"
        path.write_text(json.dumps(self.value))
        proc = subprocess.run([sys.executable, "images/guest/cache.py", "--lock", str(path), "--out", str(self.root / "bad-out")], capture_output=True, text=True)
        self.assertEqual(proc.returncode, 2)
        self.assertIn("invalid OCI archive", proc.stderr)
        self.assertNotIn("Traceback", proc.stderr)
        self.assertFalse((self.root / "bad-out").exists())

    def test_link_member(self):
        c = archive(self.root, "linked")
        path = self.root / c["archive"]
        with tarfile.open(path, "a") as tar:
            info = tarfile.TarInfo("blobs/sha256/" + "0" * 64)
            info.type, info.linkname = tarfile.SYMTYPE, "/etc/passwd"
            tar.addfile(info)
        c["archiveSha256"] = sha(path.read_bytes())
        with self.assertRaises(ValueError):
            inspect_archive(c, self.root, "amd64")

    def test_blob_hash_and_descriptor_size(self):
        for case in ["blob", "size"]:
            c = archive(self.root, case)
            path = self.root / c["archive"]
            with tarfile.open(path) as tar:
                entries = {m.name: tar.extractfile(m).read() for m in tar.getmembers()}
            if case == "blob":
                manifest = json.loads(entries["blobs/sha256/" + c["manifestDigest"][7:]])
                key = "blobs/sha256/" + manifest["layers"][0]["digest"][7:]
                entries[key] = b"x" * len(entries[key])
            else:
                index = json.loads(entries["index.json"])
                index["manifests"][0]["size"] += 1
                entries["index.json"] = encoded(index)
            with tarfile.open(path, "w") as tar:
                for name, data in entries.items():
                    info = tarfile.TarInfo(name); info.size = len(data); tar.addfile(info, io.BytesIO(data))
            c["archiveSha256"] = sha(path.read_bytes())
            with self.subTest(case=case), self.assertRaisesRegex(ValueError, "hash|size"):
                inspect_archive(c, self.root, "amd64")

    def test_wrong_config_and_missing_blob(self):
        c = self.value["components"][0]
        wrong = copy.deepcopy(c); wrong["configDigest"] = "sha256:" + "0" * 64
        with self.assertRaises(ValueError):
            inspect_archive(wrong, self.root, "amd64")
        path = self.root / c["archive"]
        with tarfile.open(path) as tar:
            entries = {m.name: tar.extractfile(m).read() for m in tar.getmembers()}
        missing = "blobs/sha256/" + c["configDigest"].split(":")[1]
        del entries[missing]
        with tarfile.open(path, "w") as tar:
            for name, data in entries.items():
                info = tarfile.TarInfo(name); info.size = len(data); tar.addfile(info, io.BytesIO(data))
        c["archiveSha256"] = sha(path.read_bytes())
        with self.assertRaises(ValueError):
            inspect_archive(c, self.root, "amd64")

    def test_interrupted_receipt_is_not_a_completion_marker(self):
        original = Path.write_bytes

        def interrupted(path, data):
            if path.name.startswith("collection.json"):
                original(path, data[:1])
                raise OSError("injected receipt I/O failure")
            return original(path, data)

        out = self.root / "out"
        with patch.object(Path, "write_bytes", interrupted), self.assertRaisesRegex(OSError, "injected"):
            collect(self.value, out, source=self.root)
        self.assertFalse((out / "collection.json").exists())

    def test_output_format_can_encode_large_layer_headers(self):
        from guest.cache import write_archive
        component = self.value["components"][0]
        inspected = inspect_archive(component, self.root, "amd64")
        with patch("guest.cache.tarfile.open", wraps=tarfile.open) as opened:
            write_archive(component, inspected, self.root, self.root / "header-test.tar", self.value["nodeLocalRef"])
        formats = [call.kwargs["format"] for call in opened.call_args_list if "format" in call.kwargs]
        self.assertEqual(len(formats), 1)
        info = tarfile.TarInfo("blobs/sha256/" + "0" * 64)
        info.size = 8 * 1024**3
        self.assertTrue(info.tobuf(format=formats[0]))

    def test_documented_cache_cli(self):
        path = self.root / "input.json"
        path.write_text(json.dumps(self.value))
        proc = subprocess.run([sys.executable, "images/guest/cache.py", "--lock", str(path), "--out", str(self.root / "cli-out")], capture_output=True, text=True)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue((self.root / "cli-out/collection.json").is_file())

    def test_existing_output_is_not_overwritten(self):
        out = self.root / "out"; out.mkdir(); (out / "sentinel").write_text("keep")
        with self.assertRaises((ValueError, FileExistsError)):
            collect(self.value, out, source=self.root)
        self.assertEqual((out / "sentinel").read_text(), "keep")

    def test_malformed_release_index_fails_cleanly(self):
        for proof in [[], {"manifests": None}, {"manifests": [{"digest": self.value["components"][0]["manifestDigest"], "platform": None}]}]:
            c = self.value["components"][0]
            data = encoded(proof)
            (self.root / "bad-proof.json").write_bytes(data)
            c.update(indexPath="bad-proof.json", indexDigest="sha256:" + sha(data))
            with self.subTest(proof=proof), self.assertRaisesRegex(ValueError, "release index"):
                inspect_archive(c, self.root, "amd64")
        path = self.root / "input.json"
        path.write_text(json.dumps(self.value))
        proc = subprocess.run([sys.executable, "images/guest/cache.py", "--lock", str(path), "--out", str(self.root / "cli-out")], capture_output=True, text=True)
        self.assertEqual(proc.returncode, 2)
        self.assertIn("release index", proc.stderr)
        self.assertNotIn("Traceback", proc.stderr)
        self.assertFalse((self.root / "cli-out").exists())

    def test_release_index_membership(self):
        c = self.value["components"][0]
        index = encoded({"schemaVersion": 2, "manifests": [{"digest": c["manifestDigest"], "platform": {"os": "linux", "architecture": "amd64"}}]})
        (self.root / "release-index.json").write_bytes(index)
        c.update(indexDigest="sha256:" + sha(index), indexPath="release-index.json")
        inspect_archive(c, self.root, "amd64")
        c["indexDigest"] = "sha256:" + "0" * 64
        with self.assertRaises(ValueError):
            inspect_archive(c, self.root, "amd64")
