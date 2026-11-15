"""App admission uses synthetic final artifacts; no network, release, or credentials."""
import base64
import copy
import hashlib
import io
import json
import os
import shutil
import stat
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

from images import candidate as c


def pin(data):
    return dict(bytes=len(data), sha256=hashlib.sha256(data).hexdigest())


class Apps(unittest.TestCase):
    def setUp(self):
        self.assertTrue(hasattr(c, "apps"), "separately approved app admission is missing")
        temp = tempfile.TemporaryDirectory(); self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.tag = "bridge-preview-fixture"
        self.policy = c.load_policy()
        self.build_policy = copy.deepcopy(self.policy)
        self.run = dict(id=19, workflow_id=29, path=".github/workflows/images.yml", run_attempt=1,
                        head_sha="b" * 40, head_branch="main", status="completed", conclusion="success",
                        event="workflow_dispatch", repository=dict(id=1444972279, full_name=c.q.REPOSITORY),
                        head_repository=dict(id=1444972279, full_name=c.q.REPOSITORY))
        self.approval = dict(runId=19, workflowId=29, runAttempt=1, buildCommit="b" * 40, artifacts={})
        self.rows, self.zips, self.payloads = {}, {}, {}
        self.source = b"synthetic corresponding source, not a real runtime"
        self.env = dict(GITHUB_EVENT_NAME="workflow_dispatch", GITHUB_REF="refs/heads/main",
                        GITHUB_REPOSITORY=c.q.REPOSITORY, GITHUB_SHA="c" * 40)
        app = io.BytesIO()
        with zipfile.ZipFile(app, "w") as z:
            z.writestr("QEMU-SOURCE.txt", "QEMU corresponding source: bridge-windows-runtime-source.zip\nSHA256: "
                + pin(self.source)["sha256"] + "\nhttps://github.com/" + c.q.REPOSITORY
                + "/releases/download/" + self.tag + "/bridge-windows-runtime-source.zip\n")
        windows = app.getvalue()
        for name, files in {
            "bridge-windows-candidate": {"bridge-windows-x64.zip": windows,
                "bridge-windows-runtime-source.zip": self.source,
                "windows-before.json": self.observation("before-windows"),
                "windows-after.json": self.observation("after-windows", {"bridge-windows-x64.zip": len(windows),
                    "bridge-windows-runtime-source.zip": len(self.source)})},
            "bridge-macos-candidate": {"bridge-macos-arm64.tar.gz": b"synthetic Mac app",
                "macos-before.json": self.observation("before-macos"),
                "macos-after.json": self.observation("after-macos", {"bridge-macos-arm64.tar.gz": 17})},
        }.items():
            self.payloads.update(files)
            artifact_id = 100 + len(self.rows)
            self.replace_zip(artifact_id, list(files.items()))
            self.rows[artifact_id] = dict(id=artifact_id, name=name, expired=False,
                digest="sha256:" + hashlib.sha256(self.zips[artifact_id]).hexdigest(),
                workflow_run=dict(id=19, repository_id=1444972279, head_repository_id=1444972279, head_sha="b" * 40))
            self.approval["artifacts"][name] = dict(id=artifact_id,
                sha256=hashlib.sha256(self.zips[artifact_id]).hexdigest(), files={n: pin(d) for n, d in files.items()})
        self.policy["appCandidates"][self.tag] = self.approval
        for patcher in (patch.object(c, "load_policy", return_value=self.policy),
                        patch.object(c.q, "gh", side_effect=self.api),
                        patch.dict(os.environ, self.env, clear=True)):
            patcher.start(); self.addCleanup(patcher.stop)
        self.release, self.assets, self.downloads, self.uploads = None, {}, [], {}
        self.fault = None

    @staticmethod
    def observation(stage, files=None):
        files = files or {}
        return json.dumps(dict(stage=stage, files=files, assetCount=len(files), totalBytes=sum(files.values()), freeBytes=123456789)).encode()

    def replace_zip(self, artifact_id, files):
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, "w") as z:
            for name, data in files:
                z.writestr(name, data)
        self.zips[artifact_id] = stream.getvalue()

    def api(self, endpoint, *, paginate=False, download=None, upload=None, body=None):
        if "/contents/" in endpoint:
            path = endpoint.split("/contents/", 1)[1].split("?ref=", 1)[0]
            raw = json.dumps(self.build_policy).encode() if path == "images/release-set.json" else (c.q.ROOT / path).read_bytes()
            digest = hashlib.sha1(b"blob " + str(len(raw)).encode() + b"\0" + raw, usedforsecurity=False).hexdigest()
            return dict(type="file", encoding="base64", content=base64.b64encode(raw).decode(),
                        sha="0" * 40 if self.fault == path else digest)
        if endpoint.endswith("/actions/runs/19"): return self.run
        if "/actions/artifacts/" in endpoint:
            artifact_id = int(endpoint.split("/actions/artifacts/")[1].split("/")[0])
            if download:
                download.write_bytes(b"corrupt" if self.fault == "zip" else self.zips[artifact_id]); return
            return self.rows[artifact_id]
        if endpoint.endswith("/releases") and body:
            self.release = dict(body, id=71, published_at=None); return self.release
        if endpoint.endswith("/releases/71"):
            return dict(self.release, draft=False) if self.fault == "published-id" else self.release
        if "/releases?" in endpoint:
            return [[]] if self.fault == "stale-list" else [[self.release] if self.release else []]
        if "/git/matching-refs/" in endpoint: return [[]]
        if upload:
            from urllib.parse import parse_qs, urlsplit
            name = parse_qs(urlsplit(endpoint).query)["name"][0]
            self.uploads[name] = upload.read_bytes()
            row = dict(id=len(self.assets) + 1, name=name, size=upload.stat().st_size, state="uploaded")
            self.assets[name] = row; return row
        if "/assets?" in endpoint:
            return [list(self.assets.values()) + ([dict(id=999, name="extra", size=1, state="uploaded")] if self.fault == "extra" else [])]
        if download:
            asset_id = int(endpoint.rsplit("/", 1)[1])
            name = next(n for n, a in self.assets.items() if a["id"] == asset_id)
            self.downloads.append(asset_id)
            download.write_bytes(b"bad" if self.fault == "readback" else self.uploads[name]); return
        self.fail("unexpected API " + endpoint)

    def publish(self, name="out"):
        name += "-" + str(len(list(self.root.iterdir())))
        runtime = dict(repositoryId=1444972279, assets={"bridge-windows-runtime-source.zip": pin(self.source)})
        original = c.package.read_json
        def read(path, *args):
            return runtime if str(path).endswith("runtime-inputs.json") else original(path, *args)
        with patch.object(c.package, "read_json", side_effect=read):
            return c.apps(self.root / name, self.tag)

    def test_exact_paired_draft_target_and_all_asset_id_readbacks(self):
        self.assertEqual(self.publish(), 71)
        self.assertEqual(self.release["target_commitish"], "b" * 40)
        self.assertTrue(self.release["draft"] and self.release["prerelease"])
        expected = {*self.payloads, "manifest-macos-arm64.json", "manifest-windows-amd64.json", "image-pins.json", "size-space.json"}
        self.assertEqual(set(self.uploads), expected)
        self.assertEqual(set(self.downloads), set(range(1, 12)))
        for target in ("macos-arm64", "windows-amd64"):
            self.assertEqual(self.uploads["manifest-" + target + ".json"], (c.MANIFESTS / (target + ".json")).read_bytes())
        pins = json.loads(self.uploads["image-pins.json"])
        self.assertEqual(len(pins["assets"]), 11)
        self.assertEqual(pins["images"], self.policy["images"])
        self.assertIn("/bridge-images-preview-43e73491-r2/", pins["assetBase"])
        self.assertEqual(len(json.loads(self.uploads["size-space.json"])["observations"]), 4)
        self.assertFalse(any(n.endswith((".kernel", ".initrd")) or ".part-" in n for n in self.uploads))

    def test_stale_release_list_does_not_hide_created_app_draft(self):
        self.fault = "stale-list"
        self.assertEqual(self.publish(), 71)
        self.assertEqual(set(self.uploads), {*self.payloads, "manifest-macos-arm64.json",
            "manifest-windows-amd64.json", "image-pins.json", "size-space.json"})
        self.assertEqual(set(self.downloads), set(range(1, 12)))

    def test_created_app_id_readback_rejects_published_release_before_upload(self):
        self.fault = "published-id"
        with self.assertRaisesRegex(ValueError, "created draft identity changed"): self.publish()
        self.assertEqual(self.release["id"], 71)
        self.assertFalse(self.uploads)

    def test_missing_approval_and_untrusted_or_build_commit_environment_cannot_write(self):
        for changes in ({"GITHUB_EVENT_NAME": "push"}, {"GITHUB_REF": "refs/heads/topic"},
                        {"GITHUB_REPOSITORY": "fork/repo"}, {"GITHUB_SHA": "b" * 40}, {"GITHUB_SHA": ""}):
            with self.subTest(changes=changes), patch.dict(os.environ, changes), self.assertRaises(ValueError):
                self.publish()
            self.assertIsNone(self.release)
        self.policy["appCandidates"] = {}
        with self.assertRaises(ValueError): self.publish()
        self.assertIsNone(self.release)

    def test_approval_cannot_relabel_the_packaged_source_url_to_another_tag(self):
        self.tag = "bridge-preview-other"
        self.policy["appCandidates"][self.tag] = self.approval
        with self.assertRaisesRegex(ValueError, "source notice"):
            self.publish()
        self.assertIsNone(self.release)

    def test_approval_cannot_attach_policy_or_native_pins_from_a_later_repin(self):
        spec = self.policy["imageRelease"]
        spec["tag"] += "-repin"
        spec["title"] += "-repin"
        with self.assertRaisesRegex(ValueError, "build image policy"):
            self.publish()
        self.assertIsNone(self.release)
        self.policy["imageRelease"] = copy.deepcopy(self.build_policy["imageRelease"])
        for path in ("images/release/macos-arm64.json", "images/release/windows-amd64.json",
                     "app/src-tauri/src/local_vm_image.rs"):
            self.fault = path
            with self.subTest(path=path), self.assertRaisesRegex(ValueError, "build image bytes"):
                self.publish()
            self.assertIsNone(self.release)

    def test_run_identity_and_status_bind_to_approved_final_run(self):
        for key, value in (("id", 20), ("workflow_id", 30), ("path", ".github/workflows/other.yml"),
                           ("run_attempt", 2), ("head_sha", "d" * 40), ("head_branch", "topic"),
                           ("event", "push"), ("status", "in_progress"), ("conclusion", "failure"),
                           ("repository", dict(id=1, full_name=c.q.REPOSITORY)),
                           ("head_repository", dict(id=1444972279, full_name="fork/repo"))):
            old = self.run[key]; self.run[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError): self.publish()
            self.run[key] = old
            self.assertIsNone(self.release)

    def test_artifact_identity_digest_expiry_and_run_binding_before_mutation(self):
        row = self.rows[100]
        changes = [("id", 999), ("name", "qemu-inputs"), ("expired", True), ("digest", "sha256:" + "0" * 64)]
        changes += [("workflow_run", dict(row["workflow_run"], **{k: v})) for k, v in
                    (("id", 20), ("repository_id", 1), ("head_repository_id", 1), ("head_sha", "d" * 40))]
        for key, value in changes:
            old = row[key]; row[key] = value
            with self.subTest(key=key), self.assertRaises(ValueError): self.publish()
            row[key] = old
            self.assertIsNone(self.release)
        self.fault = "zip"
        with self.assertRaises(ValueError): self.publish()
        self.assertIsNone(self.release)

    def test_exact_artifact_shape_names_file_pins_and_fixed_qemu_source(self):
        mutations = [lambda a: a["artifacts"].pop("bridge-macos-candidate"),
                     lambda a: a["artifacts"].update(extra=copy.deepcopy(a["artifacts"]["bridge-windows-candidate"])),
                     lambda a: a["artifacts"]["bridge-windows-candidate"]["files"].pop("windows-after.json"),
                     lambda a: a["artifacts"]["bridge-windows-candidate"]["files"].update(kernel=pin(b"x")),
                     lambda a: a["artifacts"]["bridge-macos-candidate"]["files"]["bridge-macos-arm64.tar.gz"].update(bytes=18),
                     lambda a: a["artifacts"]["bridge-macos-candidate"]["files"]["bridge-macos-arm64.tar.gz"].update(sha256="0" * 64)]
        for mutate in mutations:
            a = copy.deepcopy(self.approval); mutate(a); self.policy["appCandidates"][self.tag] = a
            with self.subTest(approval=a), self.assertRaises(ValueError): self.publish()
            self.assertIsNone(self.release)
        self.policy["appCandidates"][self.tag] = self.approval
        self.source = b"wrong fixed input"
        with self.assertRaises(ValueError): self.publish()
        self.assertIsNone(self.release)

    def test_zip_extra_missing_duplicate_paths_and_nonregular_members_rejected(self):
        originals = [(n, d) for n, d in self.payloads.items() if n.startswith("windows-") or n in ("bridge-windows-x64.zip", "bridge-windows-runtime-source.zip")]
        link = zipfile.ZipInfo("bridge-windows-x64.zip"); link.create_system = 3
        link.external_attr = (stat.S_IFLNK | 0o777) << 16
        import warnings
        for files in (originals + [("../escape", b"x")], originals[1:], originals + [originals[0]],
                      [(link if n == "bridge-windows-x64.zip" else n, d) for n, d in originals]):
            with warnings.catch_warnings():
                warnings.simplefilter("ignore", UserWarning); self.replace_zip(100, files)
            digest = hashlib.sha256(self.zips[100]).hexdigest()
            self.rows[100]["digest"] = "sha256:" + digest
            self.approval["artifacts"]["bridge-windows-candidate"]["sha256"] = digest
            with self.subTest(files=[str(n) for n, _ in files]), self.assertRaises(ValueError): self.publish()
            self.assertIsNone(self.release)
            self.assertFalse((self.root / "escape").exists())

    def test_corrupt_readback_or_extra_asset_retains_failed_draft(self):
        for fault in ("readback", "extra"):
            self.fault, self.release, self.assets, self.uploads = fault, None, {}, {}
            with self.subTest(fault=fault), self.assertRaises(ValueError): self.publish(fault)
            self.assertTrue(self.release["draft"])


class Workflow(unittest.TestCase):
    def test_dispatch_choices_and_permissions_bind_native_pair_to_public_roundtrip(self):
        import re
        text = (c.q.ROOT / ".github/workflows/images.yml").read_text()
        jobs = {m[0]: m[1] for m in re.findall(r"^  ([a-z][a-z-]+):\n(.*?)(?=^  [a-z][a-z-]+:\n|\Z)", text.split("jobs:\n", 1)[1], re.M | re.S)}
        self.assertNotIn("prepare", jobs); self.assertNotIn("draft", jobs)
        self.assertNotIn("windows_candidate_tag", text)
        self.assertIn("options: [images, build-candidate, app-draft]", text)
        for name, stage in (("images", "images"), ("public-images", "build-candidate"),
                            ("qemu-inputs", "build-candidate"), ("windows-candidate", "build-candidate"),
                            ("macos-candidate", "build-candidate"), ("app-draft", "app-draft")):
            job = jobs[name]; condition = job.splitlines()[0]
            for clause in ("github.event_name == 'workflow_dispatch'", "github.ref == 'refs/heads/main'",
                           "github.repository == 'orka-agents/orka-bridge'", "inputs.stage == '" + stage + "'"):
                self.assertIn(clause, condition)
            self.assertIn("ref: ${{ github.sha }}", job)
        self.assertIn("candidate.py public --out", jobs["public-images"])
        self.assertNotIn("GH_TOKEN", jobs["public-images"])
        self.assertIn("needs: public-images", jobs["qemu-inputs"])
        for name in ("windows-candidate", "macos-candidate"):
            job = jobs[name]
            self.assertIn("public-images", job.split("runs-on:", 1)[0])
            self.assertIn("contents: read", job); self.assertNotIn("contents: write", job)
            self.assertNotIn("GH_TOKEN", job); self.assertNotIn("github.token", job)
            self.assertIn("retention-days: 14", job)
            self.assertIn("candidate.py measure", job)
            for action in re.findall(r"uses: (\S+)", job): self.assertRegex(action, r"@[a-f0-9]{40}$")
        for name in ("images", "qemu-inputs", "app-draft"):
            self.assertIn("contents: write", jobs[name]); self.assertIn("actions: read", jobs[name])
        self.assertNotIn("npm ", jobs["qemu-inputs"])
        self.assertIn("candidate.py images --out", jobs["images"])
        self.assertIn("candidate.py apps --out", jobs["app-draft"])
        mac = jobs["macos-candidate"]
        for command in ("runs-on: macos-15", "lipo -archs", "= arm64", "build-vm-helper-macos.sh",
                        "check-vm-helper-bundle-macos.sh", "--bundles app --no-sign --no-binary-patching", "tar -czf", "public/licenses/*.txt"):
            self.assertIn(command, mac)

    def test_measure_cli_writes_observed_sizes_and_free_space_not_peak(self):
        import subprocess
        import sys
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); data = root / "app.zip"; data.write_bytes(b"test")
            report = root / "space.json"
            result = subprocess.run([sys.executable, str(c.q.ROOT / "images/candidate.py"), "measure", "--out", str(report),
                                     "--stage", "after-test", "--files", str(data)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            row = json.loads(report.read_bytes())
            self.assertEqual(row["files"], {"app.zip": 4}); self.assertEqual(row["totalBytes"], 4)
            self.assertGreater(row["freeBytes"], 0); self.assertEqual(row["stage"], "after-test")
            self.assertNotIn("peakBytes", row)
