"""Synthetic non-bootable payloads exercise packaging, never VM readiness."""
import copy
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from images import package
from images.guest.tests.fixtures import lock


def sha(data):
    return hashlib.sha256(data).hexdigest()


def metadata(root, platform="windows"):
    value = lock(root)
    if platform == "macos":
        value["architecture"] = "arm64"
        value["nodeLocalRef"] = value["nodeLocalRef"].replace("amd64", "arm64")
    return {"imageId": "synthetic-not-bootable", "buildSourceCommit": "a" * 40,
            "platform": platform, "architecture": value["architecture"],
            "format": "raw" if platform == "macos" else "qcow2",
            "minimumHostVersion": "13.0" if platform == "macos" else "11.0",
            "minimumAppVersion": "0.1.0", "stateCompatibilityId": "fixture-state-v1",
            "transportProtocolMajor": 1, "componentLock": value}


class BoundedStream(io.BytesIO):
    def read(self, size=-1):
        assert 0 < size <= 1024 * 1024, size
        return super().read(size)


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.meta = metadata(self.root)
        self.payload = b"synthetic disk, not a VM\n" * 73

    def make(self, name="package", chunk=17, payload=None):
        out = self.root / name
        result = package.create(BoundedStream(self.payload if payload is None else payload), self.meta, out, chunk)
        return out, result

    def save(self, out, value):
        (out / "manifest.json").write_bytes(package.canonical(value))

    def test_roundtrip_deterministic_streaming_and_measured_bytes(self):
        first, manifest = self.make()
        second, other = self.make("again")
        self.assertEqual(manifest, other)
        self.assertEqual({p.name: p.read_bytes() for p in first.iterdir()},
                         {p.name: p.read_bytes() for p in second.iterdir()})
        restored = self.root / "restored.disk"
        self.assertEqual(package.verify(first / "manifest.json", out=restored), manifest)
        self.assertEqual(restored.read_bytes(), self.payload)
        self.assertEqual(manifest["artifact"]["uncompressedBytes"], len(self.payload))
        self.assertEqual(manifest["artifact"]["uncompressedSha256"], sha(self.payload))
        self.assertFalse(manifest["publicationReady"])
        self.assertNotEqual(manifest["artifact"]["uncompressedBytes"], manifest["layout"]["osDisk"]["virtualCapacityBytes"])

    def test_exact_boundary_and_short_final_have_no_empty_chunk(self):
        _, initial = self.make(chunk=package.MAX_CHUNK)
        size = initial["artifact"]["compressedBytes"]
        exact, a = self.make("exact", size)
        self.assertEqual([x["bytes"] for x in a["artifact"]["chunks"]], [size])
        short, b = self.make("short", size - 1)
        self.assertEqual([x["bytes"] for x in b["artifact"]["chunks"]], [size - 1, 1])
        package.verify(exact / "manifest.json")
        package.verify(short / "manifest.json")

    def test_zero_input_and_invalid_chunk_sizes_fail_before_output(self):
        for chunk in [0, -1, True, 1.5, package.MAX_CHUNK + 1]:
            with self.subTest(chunk=chunk), self.assertRaises(ValueError):
                self.make(str(chunk), chunk)
            self.assertFalse((self.root / str(chunk)).exists())
        with self.assertRaises(ValueError):
            self.make("empty", payload=b"")
        self.assertFalse((self.root / "empty").exists())

    def test_roles_capacities_and_target_mapping(self):
        for platform, fmt, arch in [("windows", "qcow2", "amd64"), ("macos", "raw", "arm64")]:
            self.meta = metadata(self.root, platform)
            _, m = self.make(platform)
            self.assertEqual(m["target"]["architecture"], arch)
            self.assertEqual(m["layout"]["osDisk"]["format"], fmt)
            state = m["layout"]["stateDisk"]
            self.assertEqual((state["role"], state["operation"], state["format"]), ("state-disk", "create-empty", fmt))
            self.assertEqual(state["virtualCapacityBytes"], 34359738368)
            self.assertEqual(m["layout"]["osDisk"]["virtualCapacityBytes"], 17179869184)
            self.assertEqual(m["layout"]["devices"]["controlPort"], "ai.orka.bridge.control")
            self.assertEqual(m["artifact"]["role"], "os-disk")

    def test_component_identities_are_distinct_and_no_private_paths_projected(self):
        _, m = self.make()
        c = m["components"]
        self.assertIsNone(c["kmxProfileSha256"])
        self.assertEqual(c["kubernetes"]["provenance"], "policy-declared")
        self.assertEqual(c["versionPolicySha256"], sha(Path(package.guest.__file__).with_name("versions.json").read_bytes()))
        self.assertNotEqual(c["inputIdentity"], c["canonicalLockSha256"])
        text = json.dumps(m)
        for unwanted in ["chartPath", "archiveSha256", "base.qcow2", "installationUuid", "bootNonce", "credentialId"]:
            self.assertNotIn(unwanted, text)
        self.assertEqual(c["tools"][0]["name"], "kind")

    def test_bad_metadata_is_rejected_without_writing(self):
        for field, value in [("architecture", "arm64"), ("format", "raw"), ("platform", "linux"),
                             ("imageId", "../../escape"), ("imageId", "fixture..v1"), ("buildSourceCommit", "main"),
                             ("transportProtocolMajor", 2), ("transportProtocolMajor", True),
                             ("minimumAppVersion", "latest"), ("stateCompatibilityId", "")]:
            bad = copy.deepcopy(self.meta); bad[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                package.create(io.BytesIO(self.payload), bad, self.root / "bad")
            self.assertFalse((self.root / "bad").exists())
        for key in ["statePayload", "token", "localPath", "installationUuid", "publicationReady"]:
            bad = dict(self.meta, **{key: "PRIVATE_MARKER"})
            with self.subTest(key=key), self.assertRaises(ValueError):
                package.validate_metadata(bad)

    def test_corrupt_truncated_missing_extra_and_link_chunks(self):
        for kind in ["corrupt", "truncated", "missing", "extra", "symlink"]:
            out, m = self.make(kind)
            chunk = out / m["artifact"]["chunks"][0]["name"]
            if kind == "corrupt": chunk.write_bytes(b"!" * chunk.stat().st_size)
            if kind == "truncated": chunk.write_bytes(chunk.read_bytes()[:-1])
            if kind == "missing": chunk.unlink()
            if kind == "extra": (out / "unexpected").write_text("not selected")
            if kind == "symlink":
                chunk.unlink(); chunk.symlink_to(self.root / "kind")
            with self.subTest(kind=kind), self.assertRaises((ValueError, OSError)):
                package.verify(out / "manifest.json")

    def test_manifest_boundary_schema_order_duplicates_and_layout(self):
        changes = [lambda m: m.update(schemaVersion="bridge-image/v2"),
                   lambda m: m.update(publicationReady=True),
                   lambda m: m["artifact"]["chunks"].reverse(),
                   lambda m: m["artifact"]["chunks"].append(m["artifact"]["chunks"][0]),
                   lambda m: m["artifact"]["chunks"][0].update(name="../escape"),
                   lambda m: m["layout"]["stateDisk"].update(operation="reuse-initialized"),
                   lambda m: m["layout"]["osDisk"].update(virtualCapacityBytes=1),
                   lambda m: m["components"].update(token="PRIVATE_MARKER"),
                   lambda m: m["layout"]["transport"].update(protocolMajor=True)]
        for i, change in enumerate(changes):
            out, m = self.make("manifest" + str(i)); change(m); self.save(out, m)
            with self.subTest(i=i), self.assertRaises(ValueError): package.verify(out / "manifest.json")

    def test_expected_manifest_hash_is_an_external_anchor(self):
        out, _ = self.make()
        path = out / "manifest.json"
        package.verify(path, manifest_sha256=sha(path.read_bytes()))
        with self.assertRaisesRegex(ValueError, "manifest"):
            package.verify(path, manifest_sha256="0" * 64)

    def test_gzip_corruption_is_detected_even_with_updated_chunk_hashes(self):
        out, m = self.make(chunk=package.MAX_CHUNK)
        entry = m["artifact"]["chunks"][0]; path = out / entry["name"]
        data = bytearray(path.read_bytes()); data[-8] ^= 1; path.write_bytes(data)
        entry["sha256"] = m["artifact"]["compressedSha256"] = sha(data)
        self.save(out, m)
        with self.assertRaises(ValueError): package.verify(out / "manifest.json")

    def test_output_is_not_overwritten_and_failed_write_never_finishes_manifest(self):
        out, _ = self.make()
        before = (out / "manifest.json").read_bytes()
        with self.assertRaises((ValueError, OSError)): self.make()
        self.assertEqual((out / "manifest.json").read_bytes(), before)
        original = package.canonical
        def fail_final(value):
            if value.get("schemaVersion") == "bridge-image/v1":
                raise OSError("injected manifest write failure")
            return original(value)
        with patch.object(package, "canonical", side_effect=fail_final):
            with self.assertRaises(OSError): self.make("interrupted")
        self.assertTrue(list((self.root / "interrupted").glob("*.part-*")))
        self.assertFalse((self.root / "interrupted" / "manifest.json").exists())
        with self.assertRaises((ValueError, OSError)): package.verify(out / "manifest.json", out=out / "manifest.json")

    def test_reconstruction_cannot_invalidate_its_own_package_directory(self):
        out, _ = self.make()
        original_names = {p.name for p in out.iterdir()}
        with self.assertRaisesRegex(ValueError, "outside package"):
            package.verify(out / "manifest.json", out=out / "restored.disk")
        self.assertEqual({p.name for p in out.iterdir()}, original_names)
        package.verify(out / "manifest.json")

    def test_verified_output_never_replaces_a_concurrently_created_file(self):
        out, _ = self.make()
        target = self.root / "raced.disk"
        original = package.ChunkReader.readinto
        def race(reader, buffer):
            target.write_bytes(b"caller-owned")
            return original(reader, buffer)
        with patch.object(package.ChunkReader, "readinto", race), self.assertRaises(FileExistsError):
            package.verify(out / "manifest.json", out=target)
        self.assertEqual(target.read_bytes(), b"caller-owned")

    def test_external_hash_binds_the_bytes_actually_parsed(self):
        out, m = self.make()
        path = out / "manifest.json"
        replacement = dict(m, stateCompatibilityId="another-state")
        expected = sha(package.canonical(replacement))
        original = package.read_json
        def swap(path, *args, **kwargs):
            value = original(path, *args, **kwargs)
            path.write_bytes(package.canonical(replacement))
            return value
        with patch.object(package, "read_json", swap), self.assertRaisesRegex(ValueError, "manifest"):
            package.verify(path, manifest_sha256=expected)

    def test_json_duplicate_keys_and_large_metadata_are_rejected(self):
        p = self.root / "metadata.json"
        p.write_text('{"imageId":"a","imageId":"b"}')
        with self.assertRaises(ValueError): package.read_json(p)
        p.write_bytes(b" " * (package.MAX_JSON + 1))
        with self.assertRaises(ValueError): package.read_json(p)

    def test_schema_has_the_exact_top_level_contract_and_provisional_notice(self):
        out, m = self.make()
        schema = json.loads(Path(package.__file__).with_name("manifest.schema.json").read_text())
        self.assertEqual(set(schema["required"]), set(m))
        self.assertEqual(set(schema["properties"]), set(m))
        self.assertFalse(schema["additionalProperties"])
        self.assertIn("provisional", schema["$comment"].lower())
        self.assertEqual(schema["properties"]["schemaVersion"]["const"], m["schemaVersion"])
        layouts = schema["properties"]["layout"]["properties"]
        state = layouts["stateDisk"]
        self.assertEqual(set(state["required"]), set(m["layout"]["stateDisk"]))
        for field in ("role", "operation", "virtualCapacityBytes", "formatPolicy", "blockSerial"):
            self.assertEqual(state["properties"][field]["const"], m["layout"]["stateDisk"][field])
        self.assertEqual(layouts["osDisk"]["properties"]["virtualCapacityBytes"]["const"], 17179869184)
        package.verify(out / "manifest.json")

    def test_actual_cli_packages_and_verifies_without_network_or_traceback(self):
        src = self.root / "fixture.disk"; src.write_bytes(self.payload)
        meta = self.root / "metadata.json"; meta.write_text(json.dumps(self.meta))
        script = str(Path(package.__file__))
        a = subprocess.run([sys.executable, script, "--input", str(src), "--metadata", str(meta),
                            "--out", str(self.root / "cli"), "--chunk-bytes", "17"], capture_output=True, text=True)
        self.assertEqual(a.returncode, 0, a.stderr)
        b = subprocess.run([sys.executable, script, "--verify", str(self.root / "cli/manifest.json")], capture_output=True, text=True)
        self.assertEqual(b.returncode, 0, b.stderr)
        self.assertIn("not boot", b.stdout)
        bad = subprocess.run([sys.executable, script, "--verify", str(self.root / "absent.json")], capture_output=True, text=True)
        self.assertEqual(bad.returncode, 2)
        self.assertNotIn("Traceback", bad.stderr)
