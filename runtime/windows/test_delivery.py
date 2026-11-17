"""Delivery preserves verified runtime bytes and keeps corresponding source separate."""
import copy
import hashlib
import unittest
import zipfile
from pathlib import Path
from typing import Any
from unittest.mock import patch

import delivery
import test_verify
import verify


class Delivery(unittest.TestCase):
    def setUp(self):
        fixture = test_verify.Archives()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        fixture.build()
        self.inputs, self.root = fixture.output, fixture.root
        self.native = self.root / "native.rs"
        with zipfile.ZipFile(self.inputs / verify.ARCHIVES[0]) as z:
            data = z.read("provenance/runtime-manifest.json")
        self.pin: dict[str, Any] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
        self.native.write_text('fn runtime_manifest_pin() -> FilePin { FilePin { bytes: '
                               + str(self.pin["bytes"]) + ', sha256: "' + self.pin["sha256"] + '".into(), } }')
        self.policy: dict[str, Any] = {"repository": "orka-agents/orka-bridge", "repositoryId": 1444972279,
                       "producerCommit": "0" * 40, "runId": 123, "workflowId": 456,
                       "releaseId": 789, "runtimeManifest": self.pin,
                       "assets": {p.name: {"bytes": p.stat().st_size, "sha256": verify.file_hash(p)}
                                  for p in self.inputs.iterdir()}}
        self.policy["tag"] = delivery.input_tag(self.policy)
        for name, value in (("SOURCES", fixture.source_lock), ("PACKAGES", fixture.package_lock),
                            ("NATIVE_SOURCE", self.native)):
            p = patch.object(delivery, name, value)
            p.start()
            self.addCleanup(p.stop)
        self.exe = self.root / "orka-bridge.exe"
        self.exe.write_bytes(test_verify.pe())
        self.licenses = self.root / "licenses"
        self.licenses.mkdir()
        (self.licenses / "NOTICE.txt").write_bytes(b"app notice")
        self.out = self.root / "delivery"
        self.release = {"id": 789, "tag_name": self.policy["tag"], "draft": True,
                        "published_at": None, "target_commitish": "0" * 40}
        self.assets: list[dict[str, Any]] = [{"id": i + 1, "name": name, "size": row["bytes"], "state": "uploaded"}
                       for i, (name, row) in enumerate(self.policy["assets"].items())]
        self.producer_run: dict[str, Any] = {"id": 123, "workflow_id": 456, "head_sha": "0" * 40,
                    "status": "completed", "conclusion": "success", "event": "workflow_dispatch",
                    "repository": {"id": 1444972279}, "head_repository": {"id": 1444972279}}

    def gh(self, endpoint, *, paginate=False, download=None):
        if download:
            asset = next(a for a in self.assets if endpoint.endswith('/' + str(a["id"])))
            download.write_bytes((self.inputs / asset["name"]).read_bytes())
            return None
        if "/actions/runs/" in endpoint:
            return self.producer_run
        if "/matching-refs/" in endpoint:
            return [[]]
        if "/assets?" in endpoint:
            return [self.assets]
        return [[self.release]]

    def assemble(self):
        return delivery.assemble(self.inputs, self.exe, self.licenses, "preview-test", self.out, self.policy)

    def test_checked_in_policy_matches_tag_and_native_pin(self):
        p = delivery.load_policy()
        self.assertEqual(delivery.input_tag(p), p["tag"])
        self.assertEqual(len(p["assets"]), 7)
        self.assertEqual(p["runtimeManifest"]["sha256"], "a1dead19c0d1dcef558008e556cd9bd4a8c5082daf4ecea6f531d03173748385")

    def test_fetch_checks_every_file_and_stable_identity(self):
        with patch.object(delivery, "gh", side_effect=self.gh):
            delivery.fetch(self.out, self.policy)
        self.assertEqual({p.name for p in self.out.iterdir()}, set(self.policy["assets"]))
        delivery.check_inputs(self.out, self.policy)

    def test_release_identity_failures(self):
        for field, value in (("draft", False), ("published_at", "today"), ("id", 1),
                             ("target_commitish", "main")):
            with self.subTest(field=field), patch.object(delivery, "gh", side_effect=self.gh):
                original = self.release[field]
                self.release[field] = value
                with self.assertRaises(ValueError):
                    delivery.resolve_inputs(self.policy)
                self.release[field] = original

    def test_duplicate_release_wrong_ref_and_extra_asset_refused(self):
        for condition in ("duplicate", "ref", "extra"):
            def remote(endpoint, **kwargs):
                result = self.gh(endpoint, **kwargs)
                if condition == "duplicate" and endpoint.endswith("releases?per_page=100"):
                    return [[self.release, self.release]]
                if condition == "ref" and "/matching-refs/" in endpoint:
                    return [[{"ref": "refs/tags/" + self.policy["tag"],
                              "object": {"type": "commit", "sha": "f" * 40}}]]
                if condition == "extra" and "/assets?" in endpoint:
                    return [self.assets + [{"id": 999, "name": "extra", "size": 1, "state": "uploaded"}]]
                return result
            with self.subTest(condition=condition), patch.object(delivery, "gh", side_effect=remote):
                with self.assertRaises(ValueError):
                    delivery.resolve_inputs(self.policy)

    def test_fork_or_failed_run_refused(self):
        for field, value in (("conclusion", "failure"), ("head_sha", "f" * 40),
                             ("head_repository", {"id": 1})):
            original = self.producer_run[field]
            self.producer_run[field] = value
            with self.subTest(field=field), patch.object(delivery, "gh", side_effect=self.gh):
                with self.assertRaises(ValueError):
                    delivery.fetch(self.out, self.policy)
            self.producer_run[field] = original
            self.assertFalse(self.out.exists())

    def test_corruption_and_asset_change_do_not_publish_output(self):
        for mode in ("bytes", "identity"):
            calls = 0
            def remote(endpoint, **kwargs):
                nonlocal calls
                result = self.gh(endpoint, **kwargs)
                if mode == "bytes" and kwargs.get("download"):
                    kwargs["download"].write_bytes(b"corrupt")
                if "/assets?" in endpoint:
                    calls += 1
                    result = copy.deepcopy(result)
                    if mode == "identity" and calls == 2:
                        result[0][0]["id"] += 100
                return result
            with self.subTest(mode=mode), patch.object(delivery, "gh", side_effect=remote):
                with self.assertRaises(ValueError):
                    delivery.fetch(self.out, self.policy)
            self.assertFalse(self.out.exists())

    def test_counters_do_not_change_asset_identity(self):
        def remote(endpoint, **kwargs):
            result = copy.deepcopy(self.gh(endpoint, **kwargs))
            if "/assets?" in endpoint:
                for a in result[0]:
                    a.update(download_count=99, updated_at="later")
            return result
        with patch.object(delivery, "gh", side_effect=remote):
            delivery.fetch(self.out, self.policy)

    def test_separate_source_and_exact_runtime_inventory(self):
        self.assemble()
        with zipfile.ZipFile(self.out / "bridge-windows-x64.zip") as app, \
                zipfile.ZipFile(self.inputs / verify.ARCHIVES[0]) as runtime:
            expected = {"orka-bridge.exe", "licenses/NOTICE.txt", "QEMU-SOURCE.txt"}
            expected.update("native/qemu/" + n for n in runtime.namelist())
            self.assertEqual(set(app.namelist()), expected)
            for name in runtime.namelist():
                self.assertEqual(app.read("native/qemu/" + name), runtime.read(name))
            notice = app.read("QEMU-SOURCE.txt").decode()
            source = verify.ARCHIVES[1]
            self.assertIn(self.policy["assets"][source]["sha256"], notice)
            self.assertIn("https://github.com/orka-agents/orka-bridge/releases/download/preview-test/" + source, notice)
            self.assertIn("draft", notice)
            self.assertIn("asset list", notice)
        self.assertEqual(verify.file_hash(self.out / verify.ARCHIVES[1]), self.policy["assets"][verify.ARCHIVES[1]]["sha256"])

    def test_native_pin_mismatch_in_both_directions(self):
        self.native.write_text(self.native.read_text().replace(self.pin["sha256"], "f" * 64))
        with self.assertRaises(ValueError):
            self.assemble()
        self.assertFalse(self.out.exists())
        self.native.write_text(self.native.read_text().replace("f" * 64, self.pin["sha256"]))
        self.policy["runtimeManifest"]["sha256"] = "f" * 64
        with self.assertRaises(ValueError):
            self.assemble()

    def test_external_verifier_failure_is_not_bypassed(self):
        with patch.object(delivery.verify, "verify", side_effect=ValueError("external locks")):
            with self.assertRaisesRegex(ValueError, "external locks"):
                self.assemble()
        self.assertFalse(self.out.exists())

    def test_unsafe_tag_existing_output_and_corrupt_inputs(self):
        for tag in ("../bad", "x/y", "$(bad)", "", "x\n", "preview.", "preview.lock"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                delivery.validate_tag(tag)
        self.out.mkdir()
        with self.assertRaises(FileExistsError):
            self.assemble()
        self.assertEqual(list(self.out.iterdir()), [])
        (self.inputs / verify.ARCHIVES[0]).write_bytes(b"bad")
        with self.assertRaises(ValueError):
            delivery.check_inputs(self.inputs, self.policy)


class Workflow(unittest.TestCase):
    def test_reviewed_policy_agrees_with_real_native_pin(self):
        import re
        policy = delivery.load_policy()["runtimeManifest"]
        native = (delivery.HERE.parents[1] / "app/src-tauri/src/local_vm_native.rs").read_text()
        match = re.search(r'fn runtime_manifest_pin\(\).*?bytes:\s*(\d+),\s*sha256:\s*"([0-9a-f]{64})"', native, re.DOTALL)
        self.assertIsNotNone(match)
        assert match is not None
        self.assertEqual(int(match[1]), policy["bytes"])
        self.assertEqual(match[2], policy["sha256"])

    def test_candidate_path_is_dispatch_main_only_and_image_publisher_is_separate(self):
        root = Path(__file__).resolve().parents[2]
        workflow = (root / ".github/workflows/images.yml").read_text()
        for job in ("qemu-inputs", "windows-candidate"):
            block = workflow.split("\n  " + job + ":\n", 1)[1]
            condition = block.split("\n", 1)[0] if block.startswith("    if:") else ""
            for clause in ("github.event_name == 'workflow_dispatch'", "github.ref == 'refs/heads/main'",
                           "github.repository == 'orka-agents/orka-bridge'", "inputs.stage == 'build-candidate'"):
                self.assertIn(clause, condition)
        for job in ("prepare", "draft"):
            self.assertNotIn("\n  " + job + ":\n", workflow)
        candidate = workflow.split("\n  windows-candidate:\n", 1)[1].split("\n  macos-candidate:\n", 1)[0]
        self.assertIn("GIT_CONFIG_KEY_0: core.autocrlf", candidate)
        self.assertIn("GIT_CONFIG_VALUE_0: 'false'", candidate)
        self.assertIn("contents: read", candidate)
        self.assertNotIn("contents: write", candidate)
        self.assertNotIn("GH_TOKEN", candidate)
        self.assertIn("delivery.py check", candidate)
        self.assertIn("inspect-delivery.ps1", candidate)
        fetch = workflow.split("\n  qemu-inputs:\n", 1)[1].split("\n  windows-candidate:", 1)[0]
        self.assertIn("contents: write", fetch)
        self.assertIn("retention-days: 14", fetch)
        self.assertIn("compression-level: 0", fetch)
        self.assertNotIn("${{ inputs.candidate_tag }}", "\n".join(
            line for line in fetch.splitlines() if "CANDIDATE_TAG:" not in line))
        for line in fetch.splitlines():
            if "uses:" in line:
                self.assertIn("uses: actions/", line)
        trigger = (root / ".github/workflows/windows-runtime.yml").read_text()
        self.assertEqual(trigger.count("- '.github/workflows/images.yml'"), 2)


if __name__ == "__main__":
    unittest.main()
