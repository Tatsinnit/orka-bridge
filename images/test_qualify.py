"""Synthetic provenance boundaries; no network or publication evidence."""
import copy
import hashlib
import importlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
import warnings
import zipfile
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "images"))
from guest.lock import input_identity
from guest.tests.fixtures import lock

from images.test_package import boot_inputs


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
        self.boot = boot_inputs(self.root / "boot", self.metadata, self.payload)
        self.profile = dict(repository="orka-agents/orka-bridge", repositoryId=123,
                            runId=456, workflowId=789, sourceCommit="a" * 40,
                            artifactId=12, artifactSha256="0" * 64,
                            payloadPath="selected/disk.qcow2", payloadSha256=sha(self.payload),
                            metadataPath="selected/metadata.json", metadataSha256="0" * 64,
                            imageId="fixture", platform="windows", architecture="amd64",
                            nativeReceiptSha256="b" * 64,
                            inputIdentity=input_identity(self.metadata["componentLock"]))
        for role, key in (("build_receipt", "buildReceipt"), ("boot_receipt", "bootReceipt"), ("kernel", "kernel"), ("initrd", "initrd")):
            self.profile[key + "Path"] = "selected/" + self.boot[role].name
            self.profile[key + "Sha256"] = sha(self.boot[role].read_bytes())
            if role in ("kernel", "initrd"):
                self.profile[role + "Bytes"] = self.boot[role].stat().st_size
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
            for role, key in (("build_receipt", "buildReceipt"), ("boot_receipt", "bootReceipt"), ("kernel", "kernel"), ("initrd", "initrd")):
                archive.writestr(self.profile[key + "Path"], self.boot[role].read_bytes())
            archive.writestr("unselected-script.py", b"raise RuntimeError('must not execute')")
            if extra:
                archive.writestr(*extra)
        self.profile["artifactSha256"] = sha(path.read_bytes())
        return path

    def test_download_accept_header_matches_endpoint_kind(self):
        endpoints = (
            ("repos/orka-agents/orka-bridge/actions/artifacts/12/zip",
             "application/vnd.github+json"),
            ("repos/orka-agents/orka-bridge/releases/assets/34",
             "application/octet-stream"),
        )
        for index, (endpoint, accept) in enumerate(endpoints):
            with self.subTest(endpoint=endpoint):
                destination = self.root / f"download-{index}"

                def download(args, *, input, stdout, check):
                    self.assertEqual(args[:5], ["gh", "api", endpoint, "--method", "GET"])
                    self.assertEqual([args[i + 1] for i, arg in enumerate(args) if arg == "-H"],
                                     ["X-GitHub-Api-Version: 2022-11-28", "Accept: " + accept])
                    self.assertIsNone(input)
                    self.assertTrue(check)
                    stdout.write(b"archive bytes")

                with patch.object(self.q.subprocess, "run", side_effect=download):
                    self.assertIsNone(self.q.gh(endpoint, download=destination))
                self.assertEqual(destination.read_bytes(), b"archive bytes")

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
                      sourcePreviews={}, profiles={"reviewed": selected})
        with patch.object(self.q, "read_json", return_value=policy), \
             patch.object(self.q, "file_hash", return_value="c" * 64), \
             patch.dict(self.q.VERSIONS, {"releaseProfiles": {"synthetic": {"amd64": selected['inputIdentity']}}}):
            self.assertEqual(self.q.approved_profile("reviewed"), selected)
            selected["imageId"] = "fixture..v1"
            with self.assertRaisesRegex(ValueError, "image"):
                self.q.approved_profile("reviewed")

    def test_profile_requires_exact_reviewed_input_before_network(self):
        policy = dict(schemaVersion='bridge-image-policy/v1', acceptedSchemaSha256='c' * 64,
                      sourcePreviews={'3cb1f0df36705af9ff81cb8b4e8a0f8d36ee6eea':
                                      {'amd64': self.profile['inputIdentity']}},
                      profiles={'preview': self.profile})
        with patch.object(self.q, 'read_json', return_value=policy), \
             patch.object(self.q, 'file_hash', return_value='c' * 64), \
             patch.object(self.q, 'gh', side_effect=AssertionError('network before admission')):
            self.assertEqual(self.q.approved_profile('preview'), self.profile)
            self.profile['inputIdentity'] = 'f' * 64
            with self.assertRaisesRegex(ValueError, 'input.*approved'):
                self.q.approved_profile('preview')
            policy['sourcePreviews'] = {'bad-commit': {'amd64': 'f' * 64}}
            with self.assertRaises(ValueError):
                self.q.approved_profile('preview')

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
        for field in ("metadataSha256", "payloadSha256", "buildReceiptSha256", "bootReceiptSha256", "kernelSha256", "initrdSha256"):
            previous = self.profile[field]
            self.profile[field] = "f" * 64
            with self.subTest(field=field), zipfile.ZipFile(path) as archive:
                with self.assertRaises(ValueError):
                    self.q.archive_metadata(self.profile, archive)
            self.profile[field] = previous

    def test_selected_paths_must_all_differ_before_writes(self):
        path = self.archive()
        for field in ("metadataPath", "buildReceiptPath", "bootReceiptPath", "kernelPath", "initrdPath"):
            changed = dict(self.profile, **{field: self.profile["payloadPath"]})
            with self.subTest(field=field), zipfile.ZipFile(path) as archive, self.assertRaises(ValueError):
                self.q.archive_metadata(changed, archive)

    def test_boot_profile_bytes_and_missing_members_are_checked(self):
        path = self.archive()
        for role in ("kernel", "initrd"):
            for field, value in ((role + "Path", "missing"), (role + "Bytes", 1)):
                changed = dict(self.profile, **{field: value})
                with self.subTest(field=field), zipfile.ZipFile(path) as archive, self.assertRaises(ValueError):
                    self.q.archive_metadata(changed, archive)

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
                package.create(stream, metadata, out, chunk_bytes=64, **self.boot)
        verified = package.verify(out / "manifest.json")
        self.assertEqual(verified["artifact"]["uncompressedSha256"], sha(self.payload))
        self.assertEqual({p.name for p in out.iterdir()},
                         {"manifest.json", "fixture-windows-amd64.kernel", "fixture-windows-amd64.initrd"}
                         | {c["name"] for c in verified["artifact"]["chunks"]})


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

    def test_prepare_materializes_only_selected_boot_inputs(self):
        path = self.archive()
        self.artifact["digest"] = "sha256:" + self.profile["artifactSha256"]
        def api(endpoint, **kwargs):
            if endpoint.endswith("/zip"):
                kwargs["download"].write_bytes(path.read_bytes())
                return None
            return self.api(endpoint, **kwargs)
        # Only the existing production policy gate is replaced; no input/file verifier.
        with patch.object(self.q, "gh", side_effect=api), patch.object(self.q, "validate_lock"):
            out = self.root / "package"
            manifest = self.q.prepare(self.profile, out)
            self.assertEqual(manifest["directBoot"]["provenance"]["bootReceiptSha256"], self.profile["bootReceiptSha256"])
            self.assertEqual({p.name for p in out.iterdir()}, {"manifest.json", "fixture-windows-amd64.kernel", "fixture-windows-amd64.initrd",
                             *(c["name"] for c in manifest["artifact"]["chunks"])})

    def test_prepare_boot_profile_mismatch_refuses_before_output(self):
        path = self.archive()
        self.artifact["digest"] = "sha256:" + self.profile["artifactSha256"]
        def api(endpoint, **kwargs):
            if endpoint.endswith("/zip"):
                kwargs["download"].write_bytes(path.read_bytes())
                return None
            return self.api(endpoint, **kwargs)
        with patch.object(self.q, "gh", side_effect=api), patch.object(self.q, "validate_lock"):
            for field in ("bootReceiptSha256", "buildReceiptSha256", "kernelSha256", "initrdSha256"):
                previous = self.profile[field]; self.profile[field] = "f" * 64
                with self.subTest(field=field), self.assertRaisesRegex(ValueError, "checksum"):
                    self.q.prepare(self.profile, self.root / "never-created")
                self.assertFalse((self.root / "never-created").exists())
                self.profile[field] = previous

    def test_main_draft_requalifies_profile_and_refuses_foreign_prepared_context(self):
        path = self.archive()
        self.artifact["digest"] = "sha256:" + self.profile["artifactSha256"]
        def api(endpoint, **kwargs):
            if endpoint.endswith("/zip"):
                kwargs["download"].write_bytes(path.read_bytes())
                return None
            return self.api(endpoint, **kwargs)  # Rejects any release/upload endpoint.
        with patch.object(self.q, "gh", side_effect=api), patch.object(self.q, "validate_lock"):
            self.q.prepare(self.profile, self.root / "expected")
            expected = sha((self.root / "expected/manifest.json").read_bytes())
            self.metadata["stateCompatibilityId"] = "foreign-valid-metadata"
            path = self.archive()
            self.artifact["digest"] = "sha256:" + self.profile["artifactSha256"]
            args = ["qualify.py", "draft", "--profile", "synthetic", "--out", str(self.root / "requalified"),
                    "--manifest-sha256", expected]
            trusted = dict(GITHUB_EVENT_NAME="workflow_dispatch", GITHUB_REF="refs/heads/main", GITHUB_REPOSITORY=self.q.REPOSITORY)
            with patch.object(self.q, "approved_profile", return_value=self.profile), patch.object(sys, "argv", args), patch.dict(os.environ, trusted):
                self.assertEqual(self.q.main(), 2)
            value = json.loads((self.root / "requalified/manifest.json").read_bytes())
            self.assertEqual(value["stateCompatibilityId"], "foreign-valid-metadata")
            self.assertNotEqual(sha((self.root / "requalified/manifest.json").read_bytes()), expected)

    def test_prepare_source_fixture_never_becomes_a_production_package(self):
        path = self.archive()
        self.artifact["digest"] = "sha256:" + self.profile["artifactSha256"]
        def api(endpoint, **kwargs):
            if endpoint.endswith("/zip"):
                kwargs["download"].write_bytes(path.read_bytes())
                return None
            return self.api(endpoint, **kwargs)
        with patch.object(self.q, "gh", side_effect=api):
            with self.assertRaisesRegex(ValueError, "preview.*not approved"):
                self.q.prepare(self.profile, self.root / "never-created")
        self.assertFalse((self.root / "never-created").exists())

    def input_draft(self):
        data = self.archive().read_bytes()
        # The Actions artifact is the endpoint, not this disk archive.
        self.profile['artifactSha256'] = '0' * 64
        self.artifact['digest'] = 'sha256:' + '0' * 64
        pieces = [data[:len(data)//2], data[len(data)//2:]]
        self.profile['inputDraft'] = dict(
            releaseId=51, tag='bridge-image-inputs-amd64-' + sha(data),
            archiveBytes=len(data), archiveSha256=sha(data),
            parts=[dict(name=f'archive.zip.part-{i:06d}', bytes=len(b), sha256=sha(b))
                   for i, b in enumerate(pieces)])
        self.release = dict(id=51, tag_name=self.profile['inputDraft']['tag'], draft=True,
                            published_at=None, target_commitish='a' * 40)
        self.input_assets = [dict(id=100+i, name=p['name'], size=p['bytes'], state='uploaded', download_count=0)
                             for i, p in enumerate(self.profile['inputDraft']['parts'])]
        self.input_pieces = pieces

    def input_api(self, endpoint, **kwargs):
        if endpoint.endswith('/releases?per_page=100'):
            return [[copy.deepcopy(self.release)]]
        if '/git/matching-refs/' in endpoint:
            return [[]]
        if endpoint.endswith('/releases/51/assets?per_page=100'):
            return [copy.deepcopy(self.input_assets)]
        if '/releases/assets/' in endpoint:
            index = int(endpoint.rsplit('/', 1)[1]) - 100
            self.input_assets[index]['download_count'] += 1
            kwargs['download'].write_bytes(self.input_pieces[index])
            return None
        return self.api(endpoint, **kwargs)  # Deliberately denies endpoint /zip download.

    def test_input_draft_prepares_actual_archive_not_endpoint_even_after_expiry(self):
        self.input_draft()
        self.artifact['expired'] = True
        with patch.object(self.q, 'gh', side_effect=self.input_api), patch.object(self.q, 'validate_lock'):
            manifest = self.q.prepare(self.profile, self.root/'package')
        self.assertEqual(manifest['artifact']['uncompressedSha256'], sha(self.payload))
        self.assertEqual(manifest['directBoot']['kernel']['sha256'], self.profile['kernelSha256'])
        self.assertIs(manifest['publicationReady'], False)

    def test_input_draft_rejects_identity_inventory_and_corrupt_parts(self):
        self.input_draft()
        cases = [('draft', False), ('target_commitish', 'f'*40), ('id', 99),
                 ('published_at', '2026-01-01'), ('tag_name', 'different')]
        with patch.object(self.q, 'gh', side_effect=self.input_api):
            for field, value in cases:
                previous = self.release[field]
                self.release[field] = value
                with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'input draft identity'):
                    self.q.prepare(self.profile, self.root/'never-created')
                self.release[field] = previous
            for change in ('missing', 'extra', 'wrong-size', 'corrupt'):
                before = copy.deepcopy(self.input_assets)
                pieces = self.input_pieces[:]
                if change == 'missing': self.input_assets.pop()
                if change == 'extra': self.input_assets.append(dict(id=999,name='extra',size=1,state='uploaded'))
                if change == 'wrong-size': self.input_assets[0]['size'] += 1
                if change == 'corrupt': self.input_pieces[0] = b'!' * len(self.input_pieces[0])
                message = 'input part checksum' if change == 'corrupt' else 'input.*(inventory|size)'
                with self.subTest(change=change), self.assertRaisesRegex(ValueError, message):
                    self.q.prepare(self.profile, self.root/'never-created')
                self.input_assets, self.input_pieces = before, pieces
        self.assertFalse((self.root/'never-created').exists())

    def test_input_draft_policy_denies_reordered_parts_and_output_tag_before_network(self):
        self.input_draft()
        policy = dict(schemaVersion='bridge-image-policy/v1', acceptedSchemaSha256='c'*64,
                      sourcePreviews={'3cb1f0df36705af9ff81cb8b4e8a0f8d36ee6eea':
                                      {'amd64': self.profile['inputIdentity']}}, profiles={'preview':self.profile})
        with patch.object(self.q, 'read_json', return_value=policy), \
             patch.object(self.q, 'file_hash', return_value='c'*64), \
             patch.object(self.q, 'gh', side_effect=AssertionError('network before admission')):
            self.q.approved_profile('preview')
            self.profile['inputDraft']['parts'].reverse()
            with self.assertRaisesRegex(ValueError, 'input part'): self.q.approved_profile('preview')
            self.profile['inputDraft']['parts'].reverse()
            self.profile['inputDraft']['tag'] = self.q.tag(self.profile)
            with self.assertRaisesRegex(ValueError, 'input draft tag'): self.q.approved_profile('preview')

    def test_input_draft_rejects_competing_tag_and_replaced_asset(self):
        self.input_draft()
        for change in ('tag', 'asset'):
            listings = 0
            def api(endpoint, **kwargs):
                nonlocal listings
                if change == 'tag' and '/git/matching-refs/' in endpoint:
                    return [[dict(ref='refs/tags/' + self.profile['inputDraft']['tag'],
                                  object=dict(type='commit', sha='f'*40))]]
                if endpoint.endswith('/releases/51/assets?per_page=100'):
                    listings += 1
                    if listings == 2:
                        self.input_assets[0]['id'] += 1000
                return self.input_api(endpoint, **kwargs)
            message = 'tag/source' if change == 'tag' else 'changed during download'
            with self.subTest(change=change), patch.object(self.q,'gh',side_effect=api):
                with self.assertRaisesRegex(ValueError, message):
                    self.q.prepare(self.profile,self.root/'never-created')
            self.assertFalse((self.root/'never-created').exists())

    def test_input_draft_does_not_relax_other_producer_checks(self):
        self.input_draft()
        self.artifact['expired'] = True
        for field in ('id', 'repository_id', 'head_repository_id', 'head_sha'):
            original = self.artifact['workflow_run'][field]
            self.artifact['workflow_run'][field] = 'wrong'
            with self.subTest(field=field), patch.object(self.q,'gh',side_effect=self.input_api):
                with self.assertRaisesRegex(ValueError, 'artifact source'): self.q.prepare(self.profile,self.root/'never-created')
            self.artifact['workflow_run'][field] = original
        self.artifact['digest'] = 'sha256:' + 'f'*64
        with patch.object(self.q,'gh',side_effect=self.input_api), self.assertRaisesRegex(ValueError, 'artifact mismatch'):
            self.q.prepare(self.profile,self.root/'never-created')

    def prepared(self):
        package = importlib.import_module("images.package")
        out = self.root / "ready"
        with zipfile.ZipFile(self.archive()) as archive:
            with archive.open(self.profile["payloadPath"]) as stream:
                package.create(stream, self.metadata, out, chunk_bytes=64, **self.boot)
        return out, sha((out / "manifest.json").read_bytes())

    def draft_api(self, corrupt_manifest=False, wrong_upload=False, wrong_tag=False, corrupt_asset=None, listed_change=None):
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
                listed = [dict(id=k, name=v[0]) for k, v in uploads.items()]
                if listed_change == "missing": listed.pop()
                if listed_change == "extra": listed.append(dict(id=9000, name="boot-assets.json"))
                return [listed]
            if "/releases/assets/" in endpoint and "download" in kwargs:
                name, data = uploads[int(endpoint.rsplit("/", 1)[1])]
                if corrupt_asset and name.endswith("." + corrupt_asset):
                    data = b"!" * len(data)
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
        with patch.object(self.q, "gh", side_effect=self.draft_api()) as calls:
            self.assertEqual(self.q.draft(self.profile, out, digest), 31)
        uploaded = {c.kwargs["upload"].name for c in calls.call_args_list if "upload" in c.kwargs}
        downloaded = {c.kwargs["download"].name for c in calls.call_args_list if "download" in c.kwargs}
        expected = {p.name for p in out.iterdir()}
        self.assertIn("fixture-windows-amd64.kernel", expected)
        self.assertIn("fixture-windows-amd64.initrd", expected)
        self.assertEqual(uploaded, expected)
        self.assertEqual(downloaded, expected)

    def test_direct_draft_checks_new_boot_profile_pins_before_network(self):
        out, digest = self.prepared()
        fields = (("payloadSha256", "f" * 64), ("bootReceiptSha256", "f" * 64),
                  ("buildReceiptSha256", "f" * 64), ("kernelSha256", "f" * 64),
                  ("initrdSha256", "f" * 64), ("kernelBytes", 1), ("initrdBytes", 1))
        with patch.object(self.q, "gh", side_effect=AssertionError("network before context check")):
            for field, value in fields:
                with self.subTest(field=field), self.assertRaises(ValueError):
                    self.q.draft(dict(self.profile, **{field: value}), out, digest)

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

    def test_draft_readback_checks_boot_bytes_and_exact_asset_set(self):
        out, digest = self.prepared()
        for options in ({"corrupt_asset": "kernel"}, {"corrupt_asset": "initrd"},
                        {"listed_change": "missing"}, {"listed_change": "extra"}):
            with self.subTest(options=options), patch.object(self.q, "gh", side_effect=self.draft_api(**options)):
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
