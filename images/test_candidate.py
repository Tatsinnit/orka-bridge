"""Synthetic paired packages and API doubles; no release or boot evidence."""
import copy
import gzip
import importlib
import io
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.error import URLError
from urllib.parse import parse_qs, urlsplit

from images import package, qualify
from images.test_package import boot_inputs, metadata


class CandidateTests(unittest.TestCase):
    def setUp(self):
        try:
            self.c = importlib.import_module("images.candidate")
        except ModuleNotFoundError:
            self.fail("paired candidate implementation is missing")
        temp = tempfile.TemporaryDirectory(); self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.p = self.c.load_policy()
        self.real = self.c.inventory(self.p)
        self.p = copy.deepcopy(self.p)
        self.p["imageRelease"].update(tag="bridge-images-synthetic-not-bootable",
                                      title="Bridge VM images — synthetic-not-bootable", sourceCommit="a" * 40)
        pins = self.root / "pins"; pins.mkdir()
        self.files, profiles = {}, {}
        for target, entry in self.p["images"].items():
            meta = metadata(self.root, target.split("-")[0]); payload = b"synthetic disk, not a VM\n" * 73
            inputs = boot_inputs(self.root / target, meta, payload)
            count = 3 if target == "macos-arm64" else 2
            out = self.root / (target + "-package")
            m = package.create(io.BytesIO(payload), meta, out,
                               (len(gzip.compress(payload, mtime=0)) + count - 1) // count, **inputs)
            shutil.copyfile(out / "manifest.json", pins / (target + ".json"))
            entry["manifestSha256"] = qualify.file_hash(out / "manifest.json")
            profiles[entry["profile"]] = dict(imageId=m["imageId"], sourceCommit="a" * 40,
                platform=meta["platform"], architecture=meta["architecture"], inputIdentity=m["components"]["inputIdentity"],
                payloadSha256=m["artifact"]["uncompressedSha256"],
                **{r + "Sha256": m["directBoot"]["provenance"][r + "Sha256"] for r in ("bootReceipt", "buildReceipt")},
                **{r + k: m["directBoot"][r][k.lower()] for r in ("kernel", "initrd") for k in ("Sha256", "Bytes")})
            self.files.update({entry["manifestName"] if n == "manifest.json" else n: out / n
                               for n in package.asset_names(m)})
        for patcher in (patch.object(self.c, "MANIFESTS", pins),
                        patch.object(qualify, "approved_profile", side_effect=profiles.__getitem__),
                        patch.object(self.c, "load_policy", return_value=self.p)):
            patcher.start(); self.addCleanup(patcher.stop)
        self.release, self.assets, self.downloads, self.lists = None, {}, [], 0
        self.fault = None
        self.id_fault = None

    def gh(self, endpoint, *, paginate=False, body=None, upload=None, download=None):
        if endpoint.endswith("/releases") and body:
            self.release = dict(body, id=71, published_at=None)
            if self.fault == "title": self.release["name"] = "wrong"
            return self.release
        if endpoint.endswith("/releases/71"):
            if self.fault == "unreadable-id":
                raise subprocess.CalledProcessError(1, ["gh", "api", endpoint])
            row = copy.deepcopy(self.release)
            if self.fault == "changed-release" and self.assets: row["target_commitish"] = "b" * 40
            if self.id_fault: row[self.id_fault[0]] = self.id_fault[1]
            return row
        if "/releases?" in endpoint:
            rows = [self.release] if self.release else []
            if self.release:
                if self.fault == "stale-list": rows = []
                if self.fault == "overlapping-list": rows *= 2
                if self.fault == "foreign-with-created" or (self.fault == "foreign-final" and self.lists == 2):
                    rows.append(dict(self.release, id=72))
                if self.fault == "foreign-only": rows = [dict(self.release, id=72)]
            return [rows]
        if "/git/matching-refs/" in endpoint:
            return [[{"ref": "refs/tags/" + self.p["imageRelease"]["tag"],
                      "object": {"type": "commit", "sha": "b" * 40}}]] if self.fault == "ref" or (self.fault == "changed-ref" and self.assets) else [[]]
        if upload:
            name = parse_qs(urlsplit(endpoint).query)["name"][0]
            a = dict(id=len(self.assets) + 1, name=name, size=upload.stat().st_size, state="uploaded")
            self.assets[name] = a
            return a
        if "/assets?" in endpoint:
            self.lists += 1
            values = copy.deepcopy(list(self.assets.values()))
            if self.fault == "extra": values.append(dict(id=99, name="extra", size=1, state="uploaded"))
            if self.fault == "missing": values.pop()
            if self.fault == "replaced" and self.lists == 2: values[0]["id"] = 99
            return [values]
        if download:
            name = next(a["name"] for a in self.assets.values() if a["id"] == int(endpoint.rsplit("/", 1)[1]))
            self.downloads.append(name)
            download.write_bytes(b"corrupt" if self.fault == "corrupt" else self.files[name].read_bytes())
            return
        self.fail("unexpected API endpoint " + endpoint)

    def publish(self):
        with patch.object(qualify, "gh", side_effect=self.gh):
            return self.c.exact_draft(self.p["imageRelease"], self.files, self.root,
                                      lambda root: self.c.verify_images(self.p, root, mapped=True))

    def test_exact_eleven_assets_and_manifest_mapping_roundtrip(self):
        self.assertEqual(len(self.real), 11)
        self.assertIn("manifest-macos-arm64.json", self.real)
        self.assertIn("manifest-windows-amd64.json", self.real)
        self.assertNotIn("manifest.json", self.real)
        self.assertEqual(self.publish(), 71)
        self.assertEqual(set(self.downloads), set(self.files))
        self.assertEqual(self.lists, 2)

    def test_stale_release_list_does_not_hide_created_draft(self):
        for fault in ("stale-list", "overlapping-list"):
            self.release, self.assets, self.downloads, self.lists, self.fault = None, {}, [], 0, fault
            with self.subTest(fault=fault):
                self.assertEqual(self.publish(), 71)
                self.assertEqual(set(self.downloads), set(self.files))
                self.assertEqual(self.lists, 2)

    def test_post_create_identity_fields_are_checked_by_id_before_upload(self):
        for field, value in (("id", 72), ("tag_name", "other"), ("target_commitish", "b" * 40),
                             ("name", "other"), ("draft", False), ("prerelease", False),
                             ("published_at", "2026-11-15T00:00:00Z")):
            self.release, self.assets, self.id_fault = None, {}, (field, value)
            with self.subTest(field=field):
                with self.assertRaisesRegex(ValueError, "created draft identity changed"):
                    self.publish()
                self.assertEqual(self.release["id"], 71)
                self.assertFalse(self.assets)

    def test_unreadable_created_id_retains_empty_draft(self):
        self.fault = "unreadable-id"
        with self.assertRaises(subprocess.CalledProcessError): self.publish()
        self.assertEqual(self.release["id"], 71)
        self.assertFalse(self.assets)

    def test_observed_foreign_duplicate_is_rejected_even_if_created_id_is_missing(self):
        for fault in ("foreign-with-created", "foreign-only", "foreign-final"):
            self.release, self.assets, self.lists, self.fault = None, {}, 0, fault
            with self.subTest(fault=fault), self.assertRaisesRegex(ValueError, "another release shares the candidate tag"):
                self.publish()
            self.assertEqual(self.release["id"], 71)
            if fault == "foreign-final":
                self.assertEqual(self.lists, 2)
            else:
                self.assertFalse(self.assets)

    def test_policy_allows_explicit_repin_but_rejects_changed_pin_profile_and_source(self):
        repin = copy.deepcopy(self.p)
        repin["imageRelease"].update(tag=repin["imageRelease"]["tag"] + "-retry", title=repin["imageRelease"]["title"] + "-retry")
        self.assertEqual(len(self.c.inventory(repin)), 11)
        for change in ("manifestSha256", "profile", "source"):
            p = copy.deepcopy(self.p)
            if change == "source": p["imageRelease"]["sourceCommit"] = "b" * 40
            else: p["images"]["macos-arm64"][change] = "0" * 64
            with self.subTest(change=change), self.assertRaises((ValueError, KeyError)):
                self.c.inventory(p)

    def test_no_mutation_for_release_ref_or_disk_conflict(self):
        for fault in ("release", "ref", "disk"):
            self.release = dict(self.p["imageRelease"], id=88, tag_name=self.p["imageRelease"]["tag"]) if fault == "release" else None
            self.fault = fault
            free = sum(f.stat().st_size for f in self.files.values()) + 1024**3 - (1 if fault == "disk" else 0)
            with self.subTest(fault=fault), patch.object(shutil, "disk_usage", return_value=shutil._ntuple_diskusage(free, 0, free)), self.assertRaises(ValueError):
                self.publish()
            self.assertFalse(self.assets)
            self.assertEqual(self.release["id"] if self.release else None, 88 if fault == "release" else None)

    def test_budget_refuses_two_gib_and_more_than_thousand_assets(self):
        for sizes in ([2**31], [1] * 1001):
            with self.assertRaises(ValueError): self.c.budget(sizes)
        self.c.budget([2**31 - 1] * 1000)

    def test_extra_corrupt_and_replaced_assets_leave_draft(self):
        for fault in ("extra", "missing", "corrupt", "replaced", "title", "changed-ref", "changed-release"):
            self.release, self.assets, self.lists, self.fault = None, {}, 0, fault
            with self.subTest(fault=fault), self.assertRaises(ValueError): self.publish()
            self.assertEqual(self.release["id"], 71)

    def test_images_trusted_main_and_sequential_pinned_preparation(self):
        out = self.root / "images"
        with patch.dict(os.environ, {}, clear=True), self.assertRaises(ValueError): self.c.images(out)
        self.assertFalse(out.exists())
        def prepare(profile, destination):
            source = self.root / (profile["platform"] + "-" + profile["architecture"] + "-package")
            shutil.copytree(source, destination)
        with patch.dict(os.environ, GITHUB_EVENT_NAME="workflow_dispatch", GITHUB_REF="refs/heads/main", GITHUB_REPOSITORY=qualify.REPOSITORY), patch.object(qualify, "prepare", side_effect=prepare), patch.object(qualify, "gh", side_effect=self.gh):
            self.assertEqual(self.c.images(out), 71)
        for target in self.p["images"]:
            self.assertFalse(package.verify(out / target / "manifest.json")["publicationReady"])

    def test_anonymous_download_and_public_pair_use_stable_urls(self):
        urls = []
        def fetch(url, destination, size, digest):
            urls.append(url); shutil.copyfile(self.files[url.rsplit("/", 1)[1]], destination)
        with patch.object(self.c, "anonymous", side_effect=fetch), patch.object(qualify, "gh", side_effect=AssertionError("no authenticated fallback")):
            self.c.public(self.root / "public")
        self.assertEqual(len(urls), 11)
        self.assertTrue(all(u.startswith("https://github.com/orka-agents/orka-bridge/releases/download/bridge-images-synthetic-not-bootable/") for u in urls))
        for target in self.p["images"]:
            self.assertFalse(package.verify(self.root / "public" / target / "manifest.json")["publicationReady"])

    def test_anonymous_refuses_credentials_origins_truncation_and_hashes(self):
        for url in ("http://github.com/a", "https://github.com.evil/a", "https://u:p@github.com/a", "https://api.github.com/a"):
            with self.assertRaises(ValueError): self.c.safe_url(url)
            with self.assertRaises(ValueError): self.c.SafeRedirect().redirect_request(None, None, 302, "", {}, url)
        class Response(io.BytesIO):
            def geturl(self): return "https://release-assets.githubusercontent.com/a?signature=not-logged"
        data = b"ok"
        with patch("urllib.request.OpenerDirector.open", return_value=Response(data)):
            self.c.anonymous("https://github.com/a", self.root / "success", 2, "2689367b205c16ce32ed4200942b8b8b1e262dfc70d9bc9fbc77c49699a4f1df")
        self.assertEqual((self.root / "success").read_bytes(), data)
        for data, size, digest in ((b"ok", 2, "0" * 64), (b"long", 2, "0" * 64), (b"", 2, "0" * 64)):
            with patch("urllib.request.OpenerDirector.open", return_value=Response(data)), self.assertRaises(ValueError):
                self.c.anonymous("https://github.com/a", self.root / str(len(data)), size, digest)
        with patch("urllib.request.OpenerDirector.open", side_effect=URLError("secret-url")), self.assertRaisesRegex(ValueError, "anonymous download failed"):
            self.c.anonymous("https://github.com/a", self.root / "missing", 2, "0" * 64)
