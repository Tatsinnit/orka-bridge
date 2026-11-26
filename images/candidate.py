"""Paired image-only draft and anonymous roundtrip; publication is operator-owned."""
import argparse
import base64
import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from images import package
from images import qualify as q

POLICY = q.ROOT / "images/release-set.json"
MANIFESTS = q.ROOT / "images/release"
APP_RELEASE_PAGE = q.ROOT / "docs/release-page.md"
BASE = "repos/" + q.REPOSITORY
require = q.require


def load_policy():
    p = package.read_json(POLICY)
    q.shape(p, ("schemaVersion", "imageRelease", "images", "appCandidates"))
    require(p["schemaVersion"] == "bridge-release-set/v1" and isinstance(p["appCandidates"], dict),
            "unsupported release-set policy")
    q.shape(p["imageRelease"], ("tag", "title", "sourceCommit"))
    require(q.matches(p["imageRelease"]["tag"], q.SLUG)
            and q.matches(p["imageRelease"]["sourceCommit"], r"[0-9a-f]{40}"), "invalid release identity")
    return p


def inventory(p):
    require(set(p["images"]) == {"macos-arm64", "windows-amd64"}, "paired images required")
    assets = {}
    for target, entry in p["images"].items():
        q.shape(entry, ("profile", "manifestName", "manifestSha256"))
        require(entry["profile"] == "preview-" + target
                and entry["manifestName"] == "manifest-" + target + ".json", "manifest/profile mapping mismatch")
        path = MANIFESTS / (target + ".json")
        m = package.read_json(path, entry["manifestSha256"])
        package.validate_manifest(m)
        profile = q.approved_profile(entry["profile"])
        require(m["buildSourceCommit"] == profile["sourceCommit"] == p["imageRelease"]["sourceCommit"]
                and m["imageId"] == profile["imageId"]
                and all(m["target"][k] == profile[k] for k in ("platform", "architecture"))
                and target == m["target"]["platform"] + "-" + m["target"]["architecture"]
                and m["components"]["inputIdentity"] == profile["inputIdentity"], "manifest/profile/source mismatch")
        direct = m["directBoot"]
        require(m["artifact"]["uncompressedSha256"] == profile["payloadSha256"]
                and all(direct["provenance"][r + "Sha256"] == profile[r + "Sha256"] for r in ("bootReceipt", "buildReceipt"))
                and all(direct[r][k.lower()] == profile[r + k] for r in ("kernel", "initrd") for k in ("Sha256", "Bytes")),
                "manifest boot/profile mismatch")
        # A reviewed recovery tag may change without relabeling these image bytes.
        image_set = p["imageRelease"]["tag"].removeprefix("bridge-images-")
        require(image_set != p["imageRelease"]["tag"] and q.matches(image_set, q.SLUG)
                and p["imageRelease"]["title"] == "Bridge VM images — " + image_set, "image-only presentation mismatch")
        records = {a["name"]: a for a in [*m["artifact"]["chunks"], direct["kernel"], direct["initrd"]]}
        records["manifest.json"] = dict(bytes=path.stat().st_size, sha256=entry["manifestSha256"])
        for name in package.asset_names(m):
            public_name = entry["manifestName"] if name == "manifest.json" else name
            require(public_name not in assets, "duplicate image asset")
            assets[public_name] = dict(target=target, name=name, bytes=records[name]["bytes"], sha256=records[name]["sha256"])
    require(len(assets) == 11, "expected exact eleven image assets")
    budget(a["bytes"] for a in assets.values())
    return assets


def budget(sizes):
    sizes = list(sizes)
    require(0 < len(sizes) <= 1000 and all(type(n) is int and 0 < n < 2**31 for n in sizes),
            "release exceeds asset count/size budget")
    return sum(sizes)


def receipt(stage, path, files, report=None):
    sizes = {name: file.stat().st_size for name, file in files.items()}
    data = dict(stage=stage, files=sizes, assetCount=len(sizes), totalBytes=sum(sizes.values()),
                freeBytes=shutil.disk_usage(path).free)
    line = json.dumps(data, sort_keys=True)
    print(line, flush=True)
    if report is not None:
        with report.open("x", encoding="utf-8", newline="\n") as stream:
            stream.write(line + "\n")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a") as stream:
            stream.write("\n```json\n" + line + "\n```\n")
    return data["freeBytes"]


def check_available(spec, created=None):
    if created is not None:
        release_identity(spec, q.gh(BASE + "/releases/" + str(created)), created)
    releases = [r for page in q.gh(BASE + "/releases?per_page=100", paginate=True)
                for r in page if r["tag_name"] == spec["tag"]]
    refs = [r for page in q.gh(BASE + "/git/matching-refs/tags/" + spec["tag"], paginate=True)
            for r in page if r["ref"] == "refs/tags/" + spec["tag"]]
    if created is None:
        require(not releases and not refs, "candidate tag/ref already exists; use a new tag and repin")
    else:
        # The discovery list may not include a just-created draft yet; its ID
        # readback above is authoritative. Still reject observed tag conflicts.
        require(all(r["id"] == created for r in releases), "another release shares the candidate tag")
        require(all(r["object"]["type"] == "commit" and r["object"]["sha"] == spec["sourceCommit"] for r in refs),
                "candidate tag/source changed")


def release_identity(spec, r, created):
    require(type(r["id"]) is int and r["id"] > 0 and r["id"] == created
            and r["tag_name"] == spec["tag"] and r["target_commitish"] == spec["sourceCommit"]
            and r["name"] == spec["title"] and r["draft"] is True
            and r["prerelease"] is True and r["published_at"] is None, "created draft identity changed")


def exact_draft(spec, files, out, verify, body="Provisional candidate; publication remains operator-owned."):
    """Draft-only primitive; caller admits files first and verifies readback semantics."""
    require(all(not f.is_symlink() and f.is_file() for f in files.values()), "invalid candidate file")
    total = budget(f.stat().st_size for f in files.values())
    require(receipt("before-draft", out, files) >= total + 1024**3, "insufficient readback disk space")
    hashes = {n: q.file_hash(f) for n, f in files.items()}
    check_available(spec)
    r = q.gh(BASE + "/releases", body=dict(tag_name=spec["tag"], target_commitish=spec["sourceCommit"],
             name=spec["title"], draft=True, prerelease=True, body=body))
    created = r["id"]
    release_identity(spec, r, created)
    print("Created draft " + str(created) + "; failure retains it. Recovery needs a new tag and repin.", flush=True)
    check_available(spec, created)
    assets = {}
    for name, path in files.items():
        a = q.gh("https://uploads.github.com/" + BASE + "/releases/" + str(created)
                 + "/assets?name=" + urllib.parse.quote(name, safe=""), upload=path)
        require(a["name"] == name and a["state"] == "uploaded" and a["size"] == path.stat().st_size
                and type(a["id"]) is int and a["id"] > 0 and a["id"] not in {v["id"] for v in assets.values()},
                "uploaded asset identity mismatch")
        assets[name] = {k: a[k] for k in ("id", "name", "size", "state")}

    def listed():
        rows = [a for page in q.gh(BASE + "/releases/" + str(created) + "/assets?per_page=100", paginate=True) for a in page]
        require(len(rows) == len(assets) and {a["name"]: {k: a[k] for k in ("id", "name", "size", "state")} for a in rows} == assets,
                "draft complete inventory changed")

    check_available(spec, created)
    listed()
    require(receipt("before-readback", out, files) >= total + 1024**3, "insufficient readback disk space")
    with tempfile.TemporaryDirectory(dir=out) as temporary:
        root = Path(temporary)
        for name, a in assets.items():
            path = root / name
            q.gh(BASE + "/releases/assets/" + str(a["id"]), download=path)
            require(path.stat().st_size == a["size"] and q.file_hash(path) == hashes[name], "readback checksum/size mismatch")
        receipt("after-readback", out, {n: root / n for n in assets})
        verify(root)
    listed()
    check_available(spec, created)
    return created


def verify_images(p, root, *, mapped=False):
    assets = inventory(p)
    if mapped:
        require({f.name for f in root.iterdir()} == set(assets), "readback image inventory mismatch")
        for target in p["images"]:
            (root / target).mkdir()
        for public_name, a in assets.items():
            (root / public_name).rename(root / a["target"] / a["name"])
    for target, entry in p["images"].items():
        package.verify(root / target / "manifest.json", manifest_sha256=entry["manifestSha256"])


def safe_url(url):
    parsed = urllib.parse.urlsplit(url)
    require(parsed.scheme == "https" and parsed.netloc in ("github.com", "release-assets.githubusercontent.com"),
            "anonymous URL outside approved HTTPS origins")


class SafeRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        safe_url(newurl)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def anonymous(url, destination, size, digest):
    safe_url(url)
    # No auth handlers, cookies, proxy credentials, gh or token fallback.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), SafeRedirect())
    try:
        with opener.open(url, timeout=60) as response, destination.open("xb") as output:
            safe_url(response.geturl())
            total = 0
            while data := response.read(min(package.BLOCK, size - total + 1)):
                total += len(data)
                require(total <= size, "anonymous asset exceeds pinned size")
                output.write(data)
        require(total == size and q.file_hash(destination) == digest, "anonymous asset checksum/size mismatch")
    except urllib.error.URLError:
        # Signed redirect URLs must not escape through urllib's exception text.
        raise ValueError("anonymous download failed") from None


def public(out):
    p = load_policy()
    assets = inventory(p)
    out.mkdir()
    total = budget(a["bytes"] for a in assets.values())
    require(receipt("before-public", out, {}) >= total + 1024**3, "insufficient public roundtrip disk space")
    base = "https://github.com/" + q.REPOSITORY + "/releases/download/" + p["imageRelease"]["tag"] + "/"
    for target in p["images"]:
        (out / target).mkdir()
    files = {}
    for name, a in assets.items():
        destination = out / a["target"] / a["name"]
        anonymous(base + name, destination, a["bytes"], a["sha256"])
        files[name] = destination
    verify_images(p, out)
    receipt("after-public", out, files)


def images(out):
    require(os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch"
            and os.environ.get("GITHUB_REF") == "refs/heads/main"
            and os.environ.get("GITHUB_REPOSITORY") == q.REPOSITORY, "images requires manual trusted main")
    p = load_policy()
    assets = inventory(p)
    out.mkdir()
    for target, entry in p["images"].items():
        receipt("before-input-" + target, Path(os.environ.get("RUNNER_TEMP") or tempfile.gettempdir()), {})
        q.prepare(q.approved_profile(entry["profile"]), out / target)
        # prepare's input archive/extractions are already deleted before the next platform.
        package.verify(out / target / "manifest.json", manifest_sha256=entry["manifestSha256"])
        receipt("after-package-" + target, out, {n: out / a["target"] / a["name"] for n, a in assets.items() if a["target"] == target})
    files = {n: out / a["target"] / a["name"] for n, a in assets.items()}
    return exact_draft(p["imageRelease"], files, out, lambda root: verify_images(p, root, mapped=True))


APP_FILES = {
    "bridge-windows-candidate": {"bridge-windows-x64.zip", "bridge-windows-runtime-source.zip", "windows-before.json", "windows-after.json"},
    "bridge-macos-candidate": {"bridge-macos-arm64.tar.gz", "macos-before.json", "macos-after.json"},
}


APP_PUBLIC_FILES = {"bridge-windows-x64.zip", "bridge-macos-arm64.tar.gz", "bridge-windows-runtime-source.zip"}
APP_EVIDENCE_FILES = {"image-pins.json", "manifest-macos-arm64.json", "manifest-windows-amd64.json", "size-space.json",
                      "windows-before.json", "windows-after.json", "macos-before.json", "macos-after.json"}


def validate_app_tag(tag, version=...):
    number = r"(?:0|[1-9][0-9]*)"
    identifier = r"(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][A-Za-z0-9-]*)"
    semver = number + r"\." + number + r"\." + number + "(?:-" + identifier + r"(?:\." + identifier + ")*)?"
    require(q.matches(tag, "v" + semver) and len(tag) <= 128 and not tag.endswith(".lock"),
            "invalid app candidate tag; expected v<SemVer> without build metadata")
    if version is not ...:
        require(tag[1:] == version, "app candidate tag differs from build app version")


def app_approval(p, tag):
    validate_app_tag(tag)
    require(tag in p["appCandidates"], "app candidate not independently approved")
    a = p["appCandidates"][tag]
    q.shape(a, ("runId", "workflowId", "runAttempt", "buildCommit", "artifacts"))
    require(all(type(a[k]) is int and a[k] > 0 for k in ("runId", "workflowId", "runAttempt"))
            and q.matches(a["buildCommit"], r"[0-9a-f]{40}"), "invalid app build identity")
    require(set(a["artifacts"]) == set(APP_FILES), "exact paired app artifacts required")
    ids = []
    for name, members in APP_FILES.items():
        entry = a["artifacts"][name]
        q.shape(entry, ("id", "sha256", "files"))
        require(type(entry["id"]) is int and entry["id"] > 0 and q.matches(entry["sha256"], q.HEX)
                and set(entry["files"]) == members, "invalid app artifact/files approval")
        ids.append(entry["id"])
        for row in entry["files"].values():
            q.shape(row, ("bytes", "sha256"))
            budget([row["bytes"]])
            require(q.matches(row["sha256"], q.HEX), "invalid final file checksum")
    require(len(set(ids)) == 2, "duplicate app artifact ID")
    source = package.read_json(q.ROOT / "runtime/windows/runtime-inputs.json")["assets"]["bridge-windows-runtime-source.zip"]
    require(a["artifacts"]["bridge-windows-candidate"]["files"]["bridge-windows-runtime-source.zip"] == source,
            "app source differs from fixed QEMU input policy")
    return a


def apps(out, tag):
    require(os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch"
            and os.environ.get("GITHUB_REF") == "refs/heads/main"
            and os.environ.get("GITHUB_REPOSITORY") == q.REPOSITORY, "apps requires manual trusted main")
    p = load_policy()
    assets = inventory(p)
    a = app_approval(p, tag)
    approval = os.environ.get("GITHUB_SHA", "")
    require(q.matches(approval, r"[0-9a-f]{40}") and approval != a["buildCommit"], "approval must be a later commit than build")
    run = q.gh(BASE + "/actions/runs/" + str(a["runId"]))
    repository_id = package.read_json(q.ROOT / "runtime/windows/runtime-inputs.json")["repositoryId"]
    require(run["id"] == a["runId"] and run["workflow_id"] == a["workflowId"]
            and run["path"] == ".github/workflows/images.yml" and run["run_attempt"] == a["runAttempt"]
            and run["head_sha"] == a["buildCommit"] and run["head_branch"] == "main"
            and run["event"] == "workflow_dispatch" and run["status"] == "completed" and run["conclusion"] == "success"
            and all(run[k]["id"] == repository_id and run[k]["full_name"] == q.REPOSITORY for k in ("repository", "head_repository")),
            "unqualified app build run")
    # The approval checkout may already carry the next app version. Bind only
    # to the authoritative Tauri version compiled by the original build.
    remote = q.gh(BASE + "/contents/app/src-tauri/tauri.conf.json?ref=" + a["buildCommit"])
    require(remote["type"] == "file" and remote["encoding"] == "base64", "invalid build app configuration")
    validate_app_tag(tag, q.read_json(base64.b64decode(remote["content"]))["version"])
    remote = q.gh(BASE + "/contents/images/release-set.json?ref=" + a["buildCommit"])
    require(remote["type"] == "file" and remote["encoding"] == "base64", "invalid build image policy")
    built = q.read_json(base64.b64decode(remote["content"]))
    require(all(built[k] == p[k] for k in ("imageRelease", "images")), "build image policy differs from approval")
    # appCandidates is deliberately added later; the image inputs and native
    # selector must still be byte-identical to the ones compiled by that build.
    for path in ("images/release/macos-arm64.json", "images/release/windows-amd64.json",
                 "app/src-tauri/src/local_vm_image.rs"):
        raw = (q.ROOT / path).read_bytes()
        blob = hashlib.sha1(b"blob " + str(len(raw)).encode() + b"\0" + raw, usedforsecurity=False).hexdigest()
        remote = q.gh(BASE + "/contents/" + path + "?ref=" + a["buildCommit"])
        require(remote["type"] == "file" and remote["sha"] == blob, "build image bytes differ from approval")
    out.mkdir()
    with tempfile.TemporaryDirectory(prefix="app-candidate-", dir=out) as temporary:
        root, files = Path(temporary), {}
        for name, entry in a["artifacts"].items():
            endpoint = BASE + "/actions/artifacts/" + str(entry["id"])
            artifact = q.gh(endpoint)
            source = artifact["workflow_run"]
            require(artifact["id"] == entry["id"] and artifact["name"] == name and artifact["expired"] is False
                    and artifact["digest"] == "sha256:" + entry["sha256"]
                    and source["id"] == a["runId"] and source["head_sha"] == a["buildCommit"]
                    and source["repository_id"] == source["head_repository_id"] == repository_id, "app artifact/run mismatch")
            archive = root / "artifact.zip"
            q.gh(endpoint + "/zip", download=archive)
            require(q.file_hash(archive) == entry["sha256"], "app artifact ZIP checksum mismatch")
            with zipfile.ZipFile(archive) as z:
                rows = z.infolist()
                require(len(rows) == len(entry["files"]) and {i.filename for i in rows} == set(entry["files"])
                        and all(not i.is_dir() and stat.S_IFMT(i.external_attr >> 16) in (0, stat.S_IFREG) for i in rows),
                        "unsafe or inexact app artifact members")
                for info in rows:
                    row, path = entry["files"][info.filename], root / info.filename
                    require(info.file_size == row["bytes"], "final app file size mismatch")
                    with z.open(info) as src, path.open("xb") as dst:
                        shutil.copyfileobj(src, dst, package.BLOCK)
                    require(path.stat().st_size == row["bytes"] and q.file_hash(path) == row["sha256"], "final app file checksum mismatch")
                    files[info.filename] = path
            archive.unlink()
        source_name = "bridge-windows-runtime-source.zip"
        with zipfile.ZipFile(files["bridge-windows-x64.zip"]) as app:
            notices = [i for i in app.infolist() if i.filename == "QEMU-SOURCE.txt"]
            require(len(notices) == 1 and not notices[0].is_dir() and notices[0].file_size <= 16384,
                    "missing or oversized app source notice")
            lines = app.read(notices[0]).decode("utf-8").splitlines()
            require(lines[:3] == ["QEMU corresponding source: " + source_name,
                    "SHA256: " + a["artifacts"]["bridge-windows-candidate"]["files"][source_name]["sha256"],
                    "https://github.com/" + q.REPOSITORY + "/releases/download/" + tag + "/" + source_name],
                    "app source notice differs from candidate tag/source")
        observations = []
        for platform in ("windows", "macos"):
            for boundary in ("before", "after"):
                path = files[platform + "-" + boundary + ".json"]
                require(path.stat().st_size <= 32768, "size/space receipt too large")
                row = package.read_json(path)
                q.shape(row, ("stage", "files", "assetCount", "totalBytes", "freeBytes"))
                measured = {} if boundary == "before" else {n: files[n].stat().st_size for n in APP_FILES["bridge-" + platform + "-candidate"] if not n.endswith(".json")}
                require(row["stage"] == boundary + "-" + platform and row["files"] == measured
                        and row["assetCount"] == len(measured) and row["totalBytes"] == sum(measured.values())
                        and type(row["freeBytes"]) is int and row["freeBytes"] >= 0, "invalid measured size/space receipt")
                observations.append(row)
        for target, entry in p["images"].items():
            path = root / entry["manifestName"]
            shutil.copyfile(MANIFESTS / (target + ".json"), path)
            files[path.name] = path
        base = "https://github.com/" + q.REPOSITORY + "/releases/download/" + p["imageRelease"]["tag"] + "/"
        for name, data in {"image-pins.json": dict(imageRelease=p["imageRelease"], images=p["images"], assetBase=base, assets=assets),
                           "size-space.json": dict(files={n: f.stat().st_size for n, f in files.items()}, observations=observations)}.items():
            path = root / name
            path.write_text(json.dumps(data, sort_keys=True) + "\n", encoding="utf-8")
            files[name] = path
        require(set(files) == APP_PUBLIC_FILES | APP_EVIDENCE_FILES, "app file inventory mismatch")
        evidence = out / "evidence"
        evidence.mkdir()
        for name in sorted(APP_EVIDENCE_FILES):
            shutil.copyfile(files[name], evidence / name)
        require({f.name for f in evidence.iterdir()} == APP_EVIDENCE_FILES, "app evidence inventory mismatch")
        # Evidence survives temporary app cleanup and any subsequent draft or
        # readback failure, but never enters the public release inventory.
        public_files = {name: path for name, path in files.items() if name in APP_PUBLIC_FILES}
        return exact_draft(dict(tag=tag, title="Bridge preview — " + tag, sourceCommit=a["buildCommit"]), public_files, out,
                           lambda readback: require({f.name for f in readback.iterdir()} == APP_PUBLIC_FILES,
                                                    "app readback inventory mismatch"),
                           body=APP_RELEASE_PAGE.read_text(encoding="utf-8"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("images", "public", "apps", "measure"))
    parser.add_argument("--out", type=Path, required=True, help="new output path; no retry or clobber")
    parser.add_argument("--candidate-tag")
    parser.add_argument("--stage")
    parser.add_argument("--files", type=Path, nargs="*", default=[])
    args = parser.parse_args()
    try:
        if args.command == "measure":
            require(args.stage and len({f.name for f in args.files}) == len(args.files), "measurement needs stage and unique file names")
            receipt(args.stage, args.out.parent, {f.name: f for f in args.files}, report=args.out)
        elif args.command == "apps":
            apps(args.out, args.candidate_tag)
        else:
            (images if args.command == "images" else public)(args.out)
        print("Candidate operation complete; no publication or boot readiness claimed.")
        return 0
    except (ValueError, OSError, KeyError, TypeError, zipfile.BadZipFile, subprocess.CalledProcessError) as error:
        print("image candidate: " + str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
