"""App admission uses synthetic final artifacts; no network, release, or credentials."""
import base64
import copy
import hashlib
import io
import json
import os
import re
import shlex
import shutil
import subprocess
import stat
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

from images import candidate as c


def pin(data):
    return dict(bytes=len(data), sha256=hashlib.sha256(data).hexdigest())


PUBLIC_FILES = {"bridge-windows-x64.zip", "bridge-macos-arm64.tar.gz", "bridge-windows-runtime-source.zip"}
EVIDENCE_FILES = {"image-pins.json", "manifest-macos-arm64.json", "manifest-windows-amd64.json",
                  "size-space.json", "windows-before.json", "windows-after.json", "macos-before.json", "macos-after.json"}


class AppTags(unittest.TestCase):
    def test_semver_tags_match_the_complete_app_version(self):
        for version in ("0.1.0", "1.20.300", "0.1.0-rc.1", "1.0.0-0", "1.0.0-01a", "1.0.0-alpha-beta.2"):
            with self.subTest(version=version):
                c.validate_app_tag("v" + version, version)
        c.validate_app_tag("v1.0.0-" + "a" * 121, "1.0.0-" + "a" * 121)
        for tag, version in (("v0.1.0", "0.2.0"), ("v0.1.0-rc.1", "0.1.0"),
                             ("v0.1.0", "0.1.0-rc.1"), ("v0.1.0-rc.1", "0.1.0-rc.2"),
                             ("v0.1.0", None), ("v0.1.0", "")):
            with self.subTest(tag=tag, version=version), self.assertRaisesRegex(ValueError, "version"):
                c.validate_app_tag(tag, version)

    def test_malformed_semver_and_unsafe_tags_are_rejected(self):
        for tag in (None, "", "0.1.0", "bridge-preview-fixture", "v1", "v1.2", "v01.2.3", "v1.02.3", "v1.2.03",
                    "v1.2.3-01", "v1.2.3-rc.01", "v1.2.3-", "v1.2.3-rc..1", "v1.2.3-rc_1", "v1.2.3+build",
                    "v1.2.3-rc.1+build", "v1.2.3\n", "v١.2.3", "../v1.2.3", "v1.2.3/x", "v1.2.3.lock",
                    "v1.2.3-" + "a" * 122):
            with self.subTest(tag=tag), self.assertRaisesRegex(ValueError, "tag"):
                c.validate_app_tag(tag)

    def test_real_historical_approval_is_rekeyed_without_repinning(self):
        policy = c.load_policy()
        approval = c.app_approval(policy, "v0.1.0")
        self.assertEqual(approval["buildCommit"], "4bfa919f3510e6d61aae93d83cd75e2b5deeee9a")
        self.assertNotIn("bridge-preview-20261115-1414-4bfa919f", policy["appCandidates"])


class Apps(unittest.TestCase):
    def setUp(self):
        self.assertTrue(hasattr(c, "apps"), "separately approved app admission is missing")
        temp = tempfile.TemporaryDirectory(); self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.tag = "v0.1.0"
        self.build_version = "0.1.0"
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
        self.requests = []
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
        self.requests.append((endpoint, bool(body or upload)))
        if "/contents/" in endpoint:
            self.assertTrue(endpoint.endswith("?ref=" + self.approval["buildCommit"]))
            path = endpoint.split("/contents/", 1)[1].split("?ref=", 1)[0]
            if path == "app/src-tauri/tauri.conf.json":
                raw = json.dumps(dict(version=self.build_version)).encode()
            else:
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
        self.out = self.root / name
        with patch.object(c.package, "read_json", side_effect=read):
            return c.apps(self.out, self.tag)

    def test_exact_paired_draft_target_and_all_asset_id_readbacks(self):
        self.assertEqual(self.publish(), 71)
        self.assertEqual(self.release["target_commitish"], "b" * 40)
        self.assertTrue(self.release["draft"] and self.release["prerelease"])
        self.assertEqual(self.release["body"], c.APP_RELEASE_PAGE.read_text(encoding="utf-8"))
        # A release body has no repository context, so relative links break.
        links = re.findall(r"\]\(([^)]*)\)|^\s*\[[^\]]+\]:\s*(\S+)", self.release["body"], re.MULTILINE)
        self.assertTrue(links and all((inline or reference).startswith("https://") for inline, reference in links))
        self.assertEqual(set(self.uploads), PUBLIC_FILES)
        self.assertEqual(set(self.downloads), {1, 2, 3})
        for name in PUBLIC_FILES:
            self.assertEqual(self.uploads[name], self.payloads[name])
        self.assert_evidence()

    def assert_evidence(self):
        evidence = self.out / "evidence"
        self.assertEqual({p.name for p in evidence.iterdir()}, EVIDENCE_FILES)
        for name in ("windows-before.json", "windows-after.json", "macos-before.json", "macos-after.json"):
            self.assertEqual((evidence / name).read_bytes(), self.payloads[name])
        for target in ("macos-arm64", "windows-amd64"):
            self.assertEqual((evidence / ("manifest-" + target + ".json")).read_bytes(),
                             (c.MANIFESTS / (target + ".json")).read_bytes())
        pins = json.loads((evidence / "image-pins.json").read_bytes())
        self.assertEqual(len(pins["assets"]), 11)
        self.assertEqual(pins["images"], self.policy["images"])
        self.assertIn("/bridge-images-preview-43e73491-r2/", pins["assetBase"])
        space = json.loads((evidence / "size-space.json").read_bytes())
        self.assertEqual(len(space["observations"]), 4)
        self.assertEqual(space["files"], {**{n: len(data) for n, data in self.payloads.items()},
            **{"manifest-" + t + ".json": (c.MANIFESTS / (t + ".json")).stat().st_size
               for t in ("macos-arm64", "windows-amd64")}})
        self.assertEqual({p.name for p in self.out.iterdir()}, {"evidence"})

    def test_stale_release_list_does_not_hide_created_app_draft(self):
        self.fault = "stale-list"
        self.assertEqual(self.publish(), 71)
        self.assertEqual(set(self.uploads), PUBLIC_FILES)
        self.assertEqual(set(self.downloads), {1, 2, 3})
        self.assert_evidence()

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

    def test_tag_mismatch_with_original_build_version_cannot_write(self):
        for version in ("0.2.0", "0.1.0-rc.1"):
            self.build_version = version
            with self.subTest(version=version), self.assertRaisesRegex(ValueError, "version"):
                self.publish()
            self.assertFalse(any(write for _, write in self.requests))
            self.assertFalse(self.uploads)

    def test_original_build_version_not_later_checkout_version_is_authoritative(self):
        original = c.package.read_json
        def read(path, *args):
            if str(path).endswith("tauri.conf.json"):
                return dict(version="9.9.9")
            return original(path, *args)
        with patch.object(c.package, "read_json", side_effect=read):
            self.assertEqual(self.publish(), 71)
        self.assertIn((c.BASE + "/contents/app/src-tauri/tauri.conf.json?ref=" + "b" * 40, False), self.requests)

    def test_approval_cannot_relabel_the_packaged_source_url_to_another_tag(self):
        self.tag = "v0.2.0"
        self.build_version = "0.2.0"
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
            self.assertEqual(set(self.uploads), PUBLIC_FILES)
            self.assert_evidence()


class Workflow(unittest.TestCase):
    def test_build_tag_command_enforces_checkout_version_before_public_downloads(self):
        workflow = (c.q.ROOT / ".github/workflows/images.yml").read_text()
        job = workflow.split("\n  public-images:\n", 1)[1].split("\n  qemu-inputs:\n", 1)[0]
        step = job.split("- name: Validate future app tag\n", 1)[1].split("\n      - name:", 1)[0]
        command = step.split("run: ", 1)[1].strip()
        assignment, executable, *arguments = shlex.split(command)
        self.assertEqual((assignment, executable), ("PYTHONPATH=images", "python3"))
        version = json.loads((c.q.ROOT / "app/src-tauri/tauri.conf.json").read_bytes())["version"]
        for tag, expected in (("v" + version, 0), ("v" + version + "0", 1), ("v" + version + "-rc.1", 1),
                              ("bridge-preview-fixture", 1), ("v0.1.0-01", 1)):
            with self.subTest(tag=tag):
                result = subprocess.run([sys.executable, *arguments], cwd=c.q.ROOT,
                                        env={**os.environ, "PYTHONPATH": "images", "CANDIDATE_TAG": tag},
                                        capture_output=True, text=True, check=False)
                self.assertEqual(result.returncode == 0, expected == 0, result.stderr)

    def test_app_evidence_upload_is_closed_pinned_and_retained_even_after_failure(self):
        workflow = (c.q.ROOT / ".github/workflows/images.yml").read_text()
        job = workflow.split("\n  app-draft:\n", 1)[1]
        uploads = re.findall(r"^      - (.*?)(?=^      - |\Z)", job, re.M | re.S)
        uploads = [step for step in uploads if "uses: actions/upload-artifact@" in step]
        self.assertEqual(len(uploads), 1)
        step = uploads[0]
        self.assertRegex(step, r"uses: actions/upload-artifact@[0-9a-f]{40}")
        self.assertIn("if: ${{ always() }}", step)
        self.assertIn("retention-days: 14", step)
        self.assertIn("if-no-files-found: ${{ job.status == 'success' && 'error' || 'ignore' }}", step)
        paths = {line.strip() for line in step.split("path: |\n", 1)[1].split("retention-days:", 1)[0].splitlines() if line.strip()}
        self.assertEqual(paths, {"${{ runner.temp }}/app-draft/evidence/" + name for name in EVIDENCE_FILES})

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
