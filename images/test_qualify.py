"""Synthetic provenance boundaries; no network or publication evidence."""
import copy
import hashlib
import importlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import warnings
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "images"))
from guest.tests.fixtures import lock
from guest.lock import input_identity


def sha(data):
    return hashlib.sha256(data).hexdigest()


class QualificationTests(unittest.TestCase):
    def setUp(self):
        try:
            self.q = importlib.import_module("images.qualify")
        except ModuleNotFoundError:
            self.fail("qualification CLI and provenance checks are not implemented")
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.metadata = dict(imageId="fixture", buildSourceCommit="a" * 40,
                             platform="windows", architecture="amd64", format="qcow2",
                             minimumHostVersion="10.0", minimumAppVersion="0.1.0",
                             stateCompatibilityId="fixture-state", transportProtocolMajor=1,
                             componentLock=lock(self.root))
        self.payload = b"synthetic disk, not boot qualified\n" * 100
        self.profile = dict(repository="orka-agents/orka-bridge", repositoryId=123,
                            runId=456, workflowId=789, sourceCommit="a" * 40,
                            artifactId=12, artifactSha256="0" * 64,
                            payloadPath="selected/disk.qcow2", payloadSha256=sha(self.payload),
                            metadataPath="selected/metadata.json", metadataSha256="0" * 64,
                            imageId="fixture", platform="windows", architecture="amd64",
                            nativeReceiptSha256="b" * 64,
                            inputIdentity=input_identity(self.metadata["componentLock"]))
        self.run = dict(id=456, workflow_id=789, head_sha="a" * 40, event="push",
                        status="completed", conclusion="success", repository={"id": 123},
                        head_repository={"id": 123})
        self.artifact = dict(id=12, expired=False, digest="sha256:" + "0" * 64,
                             workflow_run=dict(id=456, repository_id=123,
                                               head_repository_id=123, head_sha="a" * 40))

    def policy(self):
        raw = (ROOT / "images/guest/versions.json").read_bytes()
        return {"type": "file", "sha": hashlib.sha1(b"blob " + str(len(raw)).encode() + b"\0" + raw).hexdigest()}

    def api(self, endpoint, **kwargs):
        if "/contents/images/guest/versions.json?ref=" in endpoint:
            self.assertTrue(endpoint.endswith("ref=" + self.profile["sourceCommit"]))
            return self.policy()
        if endpoint.endswith("/actions/runs/456"):
            return copy.deepcopy(self.run)
        if endpoint.endswith("/actions/artifacts/12"):
            return copy.deepcopy(self.artifact)
        raise AssertionError("unexpected API endpoint: " + endpoint)

    def archive(self, extra=None):
        raw = json.dumps(self.metadata).encode()
        self.profile["metadataSha256"] = sha(raw)
        path = self.root / "input.zip"
        with warnings.catch_warnings(), zipfile.ZipFile(path, "w") as archive:
            warnings.simplefilter("ignore", UserWarning)
            archive.writestr(self.profile["payloadPath"], self.payload)
            archive.writestr(self.profile["metadataPath"], raw)
            archive.writestr("unselected-script.py", b"raise RuntimeError('must not execute')")
            if extra:
                archive.writestr(*extra)
        self.profile["artifactSha256"] = sha(path.read_bytes())
        return path

    def test_public_cli_denies_empty_policy_before_any_gh(self):
        # The checked-in policy must reject without creating any package directory.
        for command in ("prepare", "draft"):
            result = subprocess.run([sys.executable, str(ROOT / "images/qualify.py"), command,
                                     "--profile", "not-approved", "--out", str(self.root / command)],
                                    cwd=ROOT, capture_output=True, text=True)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertIn("not approved", result.stderr)
            self.assertNotIn("Traceback", result.stderr)
            self.assertFalse((self.root / command).exists())
        with patch.object(self.q, "gh", side_effect=AssertionError("network before policy")):
            with self.assertRaisesRegex(ValueError, "not approved"):
                self.q.approved_profile("not-approved")

    def test_profile_accepts_the_manifest_image_id_grammar(self):
        selected = dict(self.profile, imageId="fixture.v1")
        policy = dict(schemaVersion="bridge-image-policy/v1", acceptedSchemaSha256="c" * 64,
                      profiles={"reviewed": selected})
        with patch.object(self.q, "read_json", return_value=policy), \
             patch.object(self.q, "file_hash", return_value="c" * 64), \
             patch.dict(self.q.VERSIONS, {"releaseProfiles": {"synthetic": {}}}):
            self.assertEqual(self.q.approved_profile("reviewed"), selected)
            selected["imageId"] = "fixture..v1"
            with self.assertRaisesRegex(ValueError, "image"):
                self.q.approved_profile("reviewed")

    def test_remote_accepts_only_pinned_successful_nonfork_run(self):
        with patch.object(self.q, "gh", side_effect=self.api):
            self.q.validate_remote(self.profile)
        for field, value in (("id", 999), ("workflow_id", 999), ("head_sha", "c" * 40),
                             ("event", "pull_request"), ("status", "in_progress"),
                             ("conclusion", "failure"), ("repository", {"id": 999}),
                             ("head_repository", {"id": 999})):
            previous = self.run[field]
            self.run[field] = value
            with self.subTest(field=field), patch.object(self.q, "gh", side_effect=self.api):
                with self.assertRaises(ValueError):
                    self.q.validate_remote(self.profile)
            self.run[field] = previous

    def test_builder_policy_mismatch_rejects_before_artifact_download(self):
        api = self.api
        def changed(endpoint, **kwargs):
            if "/contents/" in endpoint:
                return {"type": "file", "sha": "d" * 40}
            return api(endpoint, **kwargs)
        with patch.object(self.q, "gh", side_effect=changed) as calls:
            with self.assertRaisesRegex(ValueError, "policy"):
                self.q.validate_remote(self.profile)
            self.assertFalse(any(c.args[0].endswith("/zip") for c in calls.call_args_list))

    def test_artifact_cannot_change_run_repo_source_digest_or_expiry(self):
        for field, value in (("id", 99), ("expired", True), ("digest", "sha256:" + "f" * 64)):
            original = self.artifact[field]
            self.artifact[field] = value
            with self.subTest(field=field), patch.object(self.q, "gh", side_effect=self.api):
                with self.assertRaises(ValueError):
                    self.q.validate_remote(self.profile)
            self.artifact[field] = original
        for field in ("id", "repository_id", "head_repository_id", "head_sha"):
            original = self.artifact["workflow_run"][field]
            self.artifact["workflow_run"][field] = "wrong"
            with self.subTest(field=field), patch.object(self.q, "gh", side_effect=self.api):
                with self.assertRaises(ValueError):
                    self.q.validate_remote(self.profile)
            self.artifact["workflow_run"][field] = original

    def test_selected_members_are_real_zip_hash_bound_and_never_extracted(self):
        path = self.archive()
        with zipfile.ZipFile(path) as archive:
            metadata = self.q.archive_metadata(self.profile, archive)
            self.assertEqual(metadata, self.metadata)
        self.assertFalse((self.root / "selected").exists())
        for field in ("metadataSha256", "payloadSha256"):
            previous = self.profile[field]
            self.profile[field] = "f" * 64
            with self.subTest(field=field), zipfile.ZipFile(path) as archive:
                with self.assertRaises(ValueError):
                    self.q.archive_metadata(self.profile, archive)
            self.profile[field] = previous

    def test_zip_unsafe_duplicate_symlink_or_missing_selected_member_rejected(self):
        link = zipfile.ZipInfo("symlink")
        link.create_system = 3
        link.external_attr = 0o120777 << 16
        for extra in (("../escape", b"x"), ("/absolute", b"x"), ("a\\b", b"x"),
                      (self.profile["payloadPath"], b"duplicate"), (link, b"target")):
            path = self.archive(extra)
            with self.subTest(extra=extra[0]), zipfile.ZipFile(path) as archive:
                with self.assertRaises(ValueError):
                    self.q.archive_metadata(self.profile, archive)
        path = self.archive()
        self.profile["payloadPath"] = "missing"
        with zipfile.ZipFile(path) as archive, self.assertRaises(ValueError):
            self.q.archive_metadata(self.profile, archive)

    def test_metadata_source_target_and_input_identity_are_profile_bound(self):
        self.q.bind_metadata(self.profile, self.metadata)
        for field, value in (("buildSourceCommit", "d" * 40), ("imageId", "other"),
                             ("platform", "macos"), ("architecture", "arm64")):
            changed = copy.deepcopy(self.metadata)
            changed[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.q.bind_metadata(self.profile, changed)
        self.profile["inputIdentity"] = "f" * 64
        with self.assertRaises(ValueError):
            self.q.bind_metadata(self.profile, self.metadata)

    def test_draft_tag_conflicts_on_later_pages_and_existing_ref(self):
        for releases, refs in (([[], [{"id": 99, "tag_name": "image-fixture-windows-amd64", "draft": True}]], []),
                               ([[]], [{"ref": "refs/tags/image-fixture-windows-amd64"}])):
            def api(endpoint, **kwargs):
                self.assertTrue(kwargs["paginate"])
                return [refs] if "/git/matching-refs/" in endpoint else releases
            with patch.object(self.q, "gh", side_effect=api), self.assertRaises(ValueError):
                self.q.check_available(self.profile)

    def test_postcreate_identity_requires_unique_exact_draft_id(self):
        tag = "image-fixture-windows-amd64"
        good = dict(id=31, tag_name=tag, draft=True, target_commitish="a" * 40)
        for releases in ([[good, dict(good, id=32)]], [[dict(good, draft=False)]],
                         [[dict(good, id=32)]], [[dict(good, target_commitish="f" * 40)]], [[]]):
            with self.subTest(releases=releases), patch.object(self.q, "gh", return_value=releases):
                with self.assertRaises(ValueError):
                    self.q.check_available(self.profile, created=31)

    def test_real_selected_zip_stream_packages_without_raw_output(self):
        package = importlib.import_module("images.package")
        path = self.archive()
        out = self.root / "package"
        with zipfile.ZipFile(path) as archive:
            metadata = self.q.archive_metadata(self.profile, archive)
            with archive.open(self.profile["payloadPath"]) as stream:
                package.create(stream, metadata, out, chunk_bytes=64)
        verified = package.verify(out / "manifest.json")
        self.assertEqual(verified["artifact"]["uncompressedSha256"], sha(self.payload))
        self.assertEqual({p.name for p in out.iterdir()},
                         {"manifest.json"} | {c["name"] for c in verified["artifact"]["chunks"]})


    def test_prepare_download_uses_job_temp_and_rejects_archive_digest_before_output(self):
        path = self.archive()
        self.artifact["digest"] = "sha256:" + self.profile["artifactSha256"]
        job_temp = self.root / "job-temp"
        job_temp.mkdir()
        previous = os.environ.get("RUNNER_TEMP")
        os.environ["RUNNER_TEMP"] = str(job_temp)
        try:
            def api(endpoint, **kwargs):
                if endpoint.endswith("/zip"):
                    self.assertTrue(kwargs["download"].is_relative_to(job_temp))
                    kwargs["download"].write_bytes(path.read_bytes() + b"changed")
                    return None
                return self.api(endpoint, **kwargs)
            with patch.object(self.q, "gh", side_effect=api):
                with self.assertRaisesRegex(ValueError, "ZIP checksum"):
                    self.q.prepare(self.profile, self.root / "never-created")
            self.assertFalse((self.root / "never-created").exists())
            self.assertEqual(list(job_temp.iterdir()), [])
        finally:
            if previous is None:
                del os.environ["RUNNER_TEMP"]
            else:
                os.environ["RUNNER_TEMP"] = previous

    def test_prepare_source_fixture_never_becomes_a_production_package(self):
        path = self.archive()
        self.artifact["digest"] = "sha256:" + self.profile["artifactSha256"]
        def api(endpoint, **kwargs):
            if endpoint.endswith("/zip"):
                kwargs["download"].write_bytes(path.read_bytes())
                return None
            return self.api(endpoint, **kwargs)
        with patch.object(self.q, "gh", side_effect=api):
            with self.assertRaisesRegex(ValueError, "production requires"):
                self.q.prepare(self.profile, self.root / "never-created")
        self.assertFalse((self.root / "never-created").exists())

    def prepared(self):
        package = importlib.import_module("images.package")
        out = self.root / "ready"
        with zipfile.ZipFile(self.archive()) as archive:
            with archive.open(self.profile["payloadPath"]) as stream:
                package.create(stream, self.metadata, out, chunk_bytes=64)
        return out, sha((out / "manifest.json").read_bytes())

    def draft_api(self, corrupt_manifest=False, wrong_upload=False, wrong_tag=False):
        release = dict(id=31, tag_name="image-fixture-windows-amd64", draft=True,
                       target_commitish="a" * 40)
        uploads = {}
        created = False

        def api(endpoint, **kwargs):
            nonlocal created
            if "/git/matching-refs/" in endpoint:
                return [[dict(ref="refs/tags/image-fixture-windows-amd64",
                              object=dict(type="commit", sha="d" * 40))]] if created and wrong_tag else [[]]
            if endpoint.endswith("/releases?per_page=100"):
                return [[copy.deepcopy(release)] if created else []]
            if endpoint.endswith("/releases") and "body" in kwargs:
                self.assertEqual(kwargs["body"]["draft"], True)
                self.assertEqual(kwargs["body"]["target_commitish"], "a" * 40)
                created = True
                return copy.deepcopy(release)
            if "upload" in kwargs:
                path = kwargs["upload"]
                self.assertTrue(endpoint.startswith("https://uploads.github.com/repos/orka-agents/orka-bridge/releases/31/assets?name="))
                asset_id = len(uploads) + 100
                uploads[asset_id] = (path.name, path.read_bytes())
                return dict(id=asset_id, name="wrong" if wrong_upload else path.name,
                            size=path.stat().st_size, state="uploaded")
            if endpoint.endswith("/releases/31/assets?per_page=100"):
                return [[dict(id=k, name=v[0]) for k, v in uploads.items()]]
            if "/releases/assets/" in endpoint and "download" in kwargs:
                name, data = uploads[int(endpoint.rsplit("/", 1)[1])]
                if corrupt_manifest and name == "manifest.json":
                    value = json.loads(data)
                    value["stateCompatibilityId"] = "substituted-valid-metadata"
                    data = json.dumps(value, sort_keys=True, separators=(",", ":")).encode() + b"\n"
                kwargs["download"].write_bytes(data)
                return None
            raise AssertionError("unexpected draft API: " + endpoint)
        return api

    def test_draft_roundtrip_uses_authenticated_asset_ids_and_expected_manifest(self):
        out, digest = self.prepared()
        with patch.object(self.q, "gh", side_effect=self.draft_api()):
            self.assertEqual(self.q.draft(self.profile, out, digest), 31)

    def test_concurrent_wrong_tag_after_create_denies_before_any_upload(self):
        out, digest = self.prepared()
        with patch.object(self.q, "gh", side_effect=self.draft_api(wrong_tag=True)) as calls:
            with self.assertRaisesRegex(ValueError, "tag"):
                self.q.draft(self.profile, out, digest)
            self.assertEqual(sum("body" in c.kwargs for c in calls.call_args_list), 1)
            self.assertEqual(sum("upload" in c.kwargs for c in calls.call_args_list), 0)

    def test_roundtrip_rejects_substituted_manifest_even_if_other_assets_match(self):
        out, digest = self.prepared()
        with patch.object(self.q, "gh", side_effect=self.draft_api(corrupt_manifest=True)):
            with self.assertRaises(ValueError):
                self.q.draft(self.profile, out, digest)

    def test_upload_identity_must_match_requested_name(self):
        out, digest = self.prepared()
        with patch.object(self.q, "gh", side_effect=self.draft_api(wrong_upload=True)):
            with self.assertRaisesRegex(ValueError, "asset identity"):
                self.q.draft(self.profile, out, digest)

    def test_local_manifest_hash_and_profile_conflicts_deny_before_writes(self):
        out, digest = self.prepared()
        with patch.object(self.q, "gh", side_effect=AssertionError("network before local verification")):
            with self.assertRaises(ValueError):
                self.q.draft(self.profile, out, "f" * 64)
            for field, value in (("sourceCommit", "f" * 40), ("imageId", "other"),
                                 ("platform", "macos"), ("architecture", "arm64"),
                                 ("inputIdentity", "f" * 64)):
                changed = dict(self.profile, **{field: value})
                with self.subTest(field=field), self.assertRaises(ValueError):
                    self.q.draft(changed, out, digest)


if __name__ == "__main__":
    unittest.main()
