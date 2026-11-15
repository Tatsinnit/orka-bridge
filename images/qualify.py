"""Qualify pinned build bytes and create draft-only image assets; never publish."""
import argparse
import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from images.guest.lock import (
    HEX,
    VERSIONS,
    file_hash,
    input_identity,
    matches,
    read_json,
    relative_path,
    require,
    shape,
    validate_lock,
)

REPOSITORY = "orka-agents/orka-bridge"
SLUG = r"[a-z0-9][a-z0-9.-]{0,62}"
MEMBERS = ("payload", "metadata", "buildReceipt", "bootReceipt", "kernel", "initrd")


def gh(endpoint, *, paginate=False, download=None, body=None, upload=None):
    """Only network boundary: official gh authentication, never credentials in argv."""
    args = ["gh", "api", endpoint, "--method", "POST" if body is not None or upload else "GET",
            "-H", "X-GitHub-Api-Version: 2022-11-28"]
    if paginate:
        args += ["--paginate", "--slurp"]
    if download:
        # Actions archives redirect to ZIP bytes but require the API media type.
        accept = ("application/vnd.github+json"
                  if "/actions/artifacts/" in endpoint and endpoint.endswith("/zip")
                  else "application/octet-stream")
        args += ["-H", "Accept: " + accept]
    if body is not None:
        args += ["--input", "-"]
    if upload:
        args += ["--input", str(upload), "-H", "Content-Type: application/octet-stream"]
    data = json.dumps(body).encode() if body is not None else None
    if download:
        with download.open("xb") as stream:
            subprocess.run(args, input=data, stdout=stream, check=True)
        return None
    return read_json(subprocess.run(args, input=data, stdout=subprocess.PIPE, check=True).stdout)


def publisher_policy():
    policy = read_json((ROOT / "images/publisher-policy.json").read_bytes())
    shape(policy, ("schemaVersion", "acceptedSchemaSha256", "profiles", "sourcePreviews"))
    require(policy["schemaVersion"] == "bridge-image-policy/v1", "unsupported publisher policy")
    require(isinstance(policy["profiles"], dict), "invalid profiles")
    require(isinstance(policy["sourcePreviews"], dict), "invalid source previews")
    for commit, targets in policy["sourcePreviews"].items():
        require(matches(commit, r"[0-9a-f]{40}"), "invalid preview source")
        shape(targets, (), ("amd64", "arm64"))
        require(targets and all(matches(v, HEX) for v in targets.values()), "invalid preview identity")
    return policy


def approved_profile(name):
    policy = publisher_policy()
    require(matches(policy["acceptedSchemaSha256"], HEX) and name in policy["profiles"],
            "schema/profile not approved; no network permitted")
    require(file_hash(ROOT / "images/manifest.schema.json") == policy["acceptedSchemaSha256"],
            "schema bytes not approved")
    p = policy["profiles"][name]
    # With inputDraft, the Actions fields bind the endpoint producer, not the disk ZIP.
    shape(p, ("repository", "repositoryId", "runId", "workflowId", "sourceCommit",
              "artifactId", "artifactSha256", "payloadPath", "payloadSha256",
              "metadataPath", "metadataSha256", "imageId", "platform", "architecture",
              "nativeReceiptSha256", "inputIdentity", "buildReceiptPath", "buildReceiptSha256",
              "bootReceiptPath", "bootReceiptSha256", "kernelPath", "kernelSha256", "kernelBytes",
              "initrdPath", "initrdSha256", "initrdBytes"), ("inputDraft",))
    require(p["repository"] == REPOSITORY, "unapproved repository")
    for field in ("repositoryId", "runId", "workflowId", "artifactId"):
        require(type(p[field]) is int and p[field] > 0, "invalid API identity")
    for field in ("artifactSha256", "nativeReceiptSha256", "inputIdentity", *(r + "Sha256" for r in MEMBERS)):
        require(matches(p[field], HEX), "invalid profile checksum")
    require(matches(p["sourceCommit"], r"[0-9a-f]{40}") and matches(p["imageId"], SLUG) and ".." not in p["imageId"],
            "invalid source/image identity")
    require((p["platform"], p["architecture"]) in (("macos", "arm64"), ("windows", "amd64")),
            "unsupported profile target")
    selected_members(p)
    if "inputDraft" in p:
        validate_input_draft(p)
    approvals = [policy["sourcePreviews"].get(VERSIONS["orkaSource"], {}),
                 *VERSIONS["releaseProfiles"].values()]
    require(any(a.get(p["architecture"]) == p["inputIdentity"] for a in approvals),
            "guest input identity not approved; no network permitted")
    return p


def selected_members(p):
    paths = [p[r + "Path"] for r in MEMBERS]
    for path in paths:
        relative_path(path)
    require(len(set(paths)) == len(paths), "selected members must differ")
    for role in ("kernel", "initrd"):
        require(type(p[role + "Bytes"]) is int and p[role + "Bytes"] > 0, "invalid selected boot size")
    return paths


def tag(p):
    return "image-" + "-".join(p[k] for k in ("imageId", "platform", "architecture"))


def validate_remote(p):
    base = "repos/" + p["repository"]
    run = gh(base + "/actions/runs/" + str(p["runId"]))
    require(run["id"] == p["runId"] and run["workflow_id"] == p["workflowId"]
            and run["head_sha"] == p["sourceCommit"], "run/source/workflow mismatch")
    require(run["status"] == "completed" and run["conclusion"] == "success"
            and run["event"] in ("push", "workflow_dispatch"), "unqualified source run")
    require(run["repository"]["id"] == run["head_repository"]["id"] == p["repositoryId"],
            "fork/repository mismatch")
    policy = gh(base + "/contents/images/guest/versions.json?ref=" + p["sourceCommit"])
    raw_policy = (ROOT / "images/guest/versions.json").read_bytes()
    # GitHub exposes Git blob identity; the manifest separately records policy SHA-256.
    blob = hashlib.sha1(b"blob " + str(len(raw_policy)).encode() + b"\0" + raw_policy, usedforsecurity=False).hexdigest()
    require(policy["type"] == "file" and policy["sha"] == blob, "builder/publisher version policy mismatch")
    artifact = gh(base + "/actions/artifacts/" + str(p["artifactId"]))
    require(artifact["id"] == p["artifactId"] and (artifact["expired"] is False or "inputDraft" in p)
            and artifact["digest"] == "sha256:" + p["artifactSha256"], "artifact mismatch")
    source = artifact["workflow_run"]
    require(source["id"] == p["runId"]
            and source["repository_id"] == source["head_repository_id"] == p["repositoryId"]
            and source["head_sha"] == p["sourceCommit"], "artifact source mismatch")


def validate_input_draft(p):
    d = p["inputDraft"]
    shape(d, ("releaseId", "tag", "archiveBytes", "archiveSha256", "parts"))
    require(type(d["releaseId"]) is int and d["releaseId"] > 0, "invalid input release identity")
    require(matches(d["archiveSha256"], HEX) and type(d["archiveBytes"]) is int
            and d["archiveBytes"] > 0, "invalid input archive identity")
    require(d["tag"] == "bridge-image-inputs-" + p["architecture"] + "-" + d["archiveSha256"],
            "invalid input draft tag")
    parts = d["parts"]
    require(isinstance(parts, list) and 0 < len(parts) <= 64, "invalid input archive parts")
    for i, part in enumerate(parts):
        shape(part, ("name", "bytes", "sha256"))
        require(part["name"] == f"archive.zip.part-{i:06d}" and type(part["bytes"]) is int
                and 0 < part["bytes"] <= 1024**3 and matches(part["sha256"], HEX), "invalid input part")
    require(sum(part["bytes"] for part in parts) == d["archiveBytes"], "input archive size mismatch")


def input_release(p):
    d, base = p["inputDraft"], "repos/" + p["repository"]
    releases = [r for page in gh(base + "/releases?per_page=100", paginate=True)
                for r in page if r["tag_name"] == d["tag"]]
    require(len(releases) == 1 and releases[0]["id"] == d["releaseId"]
            and releases[0]["draft"] is True and releases[0]["published_at"] is None
            and releases[0]["target_commitish"] == p["sourceCommit"], "input draft identity mismatch")
    refs = [r for page in gh(base + "/git/matching-refs/tags/" + d["tag"], paginate=True)
            for r in page if r["ref"] == "refs/tags/" + d["tag"]]
    require(all(r["object"]["type"] == "commit" and r["object"]["sha"] == p["sourceCommit"]
                for r in refs), "input draft tag/source mismatch")
    assets = [a for page in gh(base + "/releases/" + str(d["releaseId"]) + "/assets?per_page=100",
                              paginate=True) for a in page]
    require(len(assets) == len(d["parts"]) and {a["name"] for a in assets} == {v["name"] for v in d["parts"]},
            "input draft inventory mismatch")
    # Download counters and timestamps are observations, not archive identity.
    by_name = {a["name"]: {k: a[k] for k in ("id", "name", "size", "state")} for a in assets}
    require(len({a["id"] for a in assets}) == len(assets), "duplicate input asset identity")
    for part in d["parts"]:
        a = by_name[part["name"]]
        require(type(a["id"]) is int and a["id"] > 0 and a["state"] == "uploaded"
                and a["size"] == part["bytes"], "input asset identity/size mismatch")
    return by_name


def download_archive(p, path):
    base = "repos/" + p["repository"]
    if "inputDraft" not in p:
        gh(base + "/actions/artifacts/" + str(p["artifactId"]) + "/zip", download=path)
        require(file_hash(path) == p["artifactSha256"], "downloaded ZIP checksum mismatch")
        return
    validate_input_draft(p)
    assets = input_release(p)
    with path.open("xb") as archive:
        for part in p["inputDraft"]["parts"]:
            temporary = path.with_name(part["name"])
            gh(base + "/releases/assets/" + str(assets[part["name"]]["id"]), download=temporary)
            require(temporary.stat().st_size == part["bytes"] and file_hash(temporary) == part["sha256"],
                    "input part checksum/length mismatch")
            with temporary.open("rb") as source:
                shutil.copyfileobj(source, archive, 1024**2)
            temporary.unlink()
    require(path.stat().st_size == p["inputDraft"]["archiveBytes"]
            and file_hash(path) == p["inputDraft"]["archiveSha256"], "input ZIP checksum/length mismatch")
    require(input_release(p) == assets, "input draft changed during download")


def archive_metadata(p, archive):
    paths = selected_members(p)
    names = set()
    for entry in archive.infolist():
        name = entry.filename.rstrip("/") if entry.is_dir() else entry.filename
        relative_path(name)
        mode = stat.S_IFMT(entry.external_attr >> 16)
        require(name not in names and mode in (0, stat.S_IFREG, stat.S_IFDIR),
                "duplicate or nonregular ZIP member")
        names.add(name)
    for path in paths:
        require(path in names and not archive.getinfo(path).is_dir(), "missing selected ZIP member")
    for role in MEMBERS:
        info = archive.getinfo(p[role + "Path"])
        if role in ("metadata", "buildReceipt", "bootReceipt"):
            require(info.file_size <= 8 * 1024**2, "selected JSON too large")
        if role in ("kernel", "initrd"):
            require(info.file_size == p[role + "Bytes"], "selected boot size mismatch")
        with archive.open(info) as stream:
            require(hashlib.file_digest(stream, "sha256").hexdigest() == p[role + "Sha256"], "selected " + role + " checksum mismatch")
    info = archive.getinfo(p["metadataPath"])
    require(info.file_size <= 8 * 1024 * 1024, "metadata too large")
    with archive.open(info) as stream:
        raw = stream.read(8 * 1024 * 1024 + 1)
    require(len(raw) <= 8 * 1024 * 1024
            and hashlib.sha256(raw).hexdigest() == p["metadataSha256"], "metadata checksum mismatch")
    return read_json(raw)


def bind_metadata(p, metadata):
    from images import package
    package.validate_metadata(metadata)
    for field, selected in (("buildSourceCommit", "sourceCommit"), ("imageId", "imageId"),
                            ("platform", "platform"), ("architecture", "architecture")):
        require(metadata[field] == p[selected], "metadata source/target mismatch")
    require(input_identity(metadata["componentLock"]) == p["inputIdentity"], "metadata input identity mismatch")


def prepare(p, out):
    from images import package
    require(not out.exists(), "output already exists")
    validate_remote(p)
    with tempfile.TemporaryDirectory(dir=os.environ.get("RUNNER_TEMP")) as temporary:
        path = Path(temporary) / "build.zip"
        download_archive(p, path)
        with zipfile.ZipFile(path) as archive:
            metadata = archive_metadata(p, archive)
            bind_metadata(p, metadata)
            validate_lock(metadata["componentLock"], production=True,
                          source_previews=publisher_policy()["sourcePreviews"])
            inputs = {}
            for role, argument in (("buildReceipt", "build_receipt"), ("bootReceipt", "boot_receipt"), ("kernel", "kernel"), ("initrd", "initrd")):
                destination = Path(temporary) / role
                with archive.open(p[role + "Path"]) as source, destination.open("xb") as target:
                    shutil.copyfileobj(source, target, 1024**2)
                require(file_hash(destination) == p[role + "Sha256"], "selected input changed while copying")
                inputs[argument] = destination
            with archive.open(p["payloadPath"]) as stream:
                package.create(stream, metadata, out, **inputs)
    return package.verify(out / "manifest.json")


def check_available(p, created=None):
    base = "repos/" + p["repository"]
    releases = [r for page in gh(base + "/releases?per_page=100", paginate=True)
                for r in page if r["tag_name"] == tag(p)]
    if created is None:
        require(not releases, "image tag already has a release, including draft")
    else:
        require(len(releases) == 1 and releases[0]["id"] == created
                and releases[0]["draft"] is True
                and releases[0]["target_commitish"] == p["sourceCommit"],
                "created draft identity is not unique or changed")
    refs = [r for page in gh(base + "/git/matching-refs/tags/" + tag(p), paginate=True)
            for r in page if r["ref"] == "refs/tags/" + tag(p)]
    if created is None:
        require(not refs, "image tag ref already exists")
    else:
        # target_commitish is ignored by GitHub if a competing writer created the tag.
        require(all(r["object"]["type"] == "commit" and r["object"]["sha"] == p["sourceCommit"]
                    for r in refs), "created draft tag/source mismatch")


def draft(p, out, expected_sha):
    from images import package
    manifest = package.verify(out / "manifest.json", manifest_sha256=expected_sha)
    direct = manifest["directBoot"]
    require(manifest["artifact"]["uncompressedSha256"] == p["payloadSha256"]
            and all(direct["provenance"][r + "Sha256"] == p[r + "Sha256"] for r in ("bootReceipt", "buildReceipt"))
            and all(direct[r]["sha256"] == p[r + "Sha256"] and direct[r]["bytes"] == p[r + "Bytes"] for r in ("kernel", "initrd")),
            "draft boot/profile binding mismatch")
    require(manifest["imageId"] == p["imageId"]
            and manifest["buildSourceCommit"] == p["sourceCommit"]
            and all(manifest["target"][k] == p[k] for k in ("platform", "architecture"))
            and manifest["components"]["inputIdentity"] == p["inputIdentity"], "draft manifest/profile mismatch")
    check_available(p)
    base = "repos/" + p["repository"]
    release = gh(base + "/releases", body=dict(tag_name=tag(p), target_commitish=p["sourceCommit"],
                                             name=tag(p), draft=True, prerelease=True,
                                             body="Provisional image assets; not boot or publication approval."))
    release_id = release["id"]
    require(type(release_id) is int and release_id > 0 and release["draft"] is True
            and release["tag_name"] == tag(p)
            and release["target_commitish"] == p["sourceCommit"], "created draft response mismatch")
    print("Created draft release " + str(release_id) + "; failures leave it for operator inspection.", flush=True)
    check_available(p, created=release_id)
    assets = {}
    files = package.asset_names(manifest)
    for name in files:
        asset = gh("https://uploads.github.com/" + base + "/releases/" + str(release_id)
                   + "/assets?name=" + name, upload=out / name)
        require(asset["name"] == name and asset["state"] == "uploaded"
                and asset["size"] == (out / name).stat().st_size
                and type(asset["id"]) is int and asset["id"] > 0
                and asset["id"] not in assets.values(), "uploaded asset identity mismatch")
        assets[name] = asset["id"]
    listed = [a for page in gh(base + "/releases/" + str(release_id) + "/assets?per_page=100",
                              paginate=True) for a in page]
    require(len(listed) == len(assets) and {a["name"]: a["id"] for a in listed} == assets,
            "draft asset set mismatch")
    with tempfile.TemporaryDirectory(dir=os.environ.get("RUNNER_TEMP")) as temporary:
        downloaded = Path(temporary)
        for name, asset_id in assets.items():
            gh(base + "/releases/assets/" + str(asset_id), download=downloaded / name)
        package.verify(downloaded / "manifest.json", manifest_sha256=expected_sha)
    check_available(p, created=release_id)
    return release_id


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("prepare", "draft"))
    parser.add_argument("--profile", required=True, help="reviewed checked-in profile name")
    parser.add_argument("--out", type=Path, required=True, help="new compressed package directory")
    parser.add_argument("--manifest-sha256", help="prepare job's expected manifest identity (required for draft)")
    args = parser.parse_args()
    try:
        p = approved_profile(args.profile)
        if args.command == "draft":
            require(os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch"
                    and os.environ.get("GITHUB_REF") == "refs/heads/main"
                    and os.environ.get("GITHUB_REPOSITORY") == REPOSITORY, "draft requires manual trusted main")
            require(matches(args.manifest_sha256, HEX), "draft requires prepare manifest checksum")
        prepare(p, args.out)
        digest = file_hash(args.out / "manifest.json")
        if args.command == "draft":
            require(digest == args.manifest_sha256, "prepare/draft manifest mismatch")
            draft(p, args.out, args.manifest_sha256)
        if os.environ.get("GITHUB_OUTPUT"):
            with Path(os.environ["GITHUB_OUTPUT"]).open("a") as stream:
                stream.write("tag=" + tag(p) + "\nmanifest_sha256=" + digest + "\n")
        print("Verified compressed package; no publication or boot qualification claimed.")
        return 0
    except (ValueError, OSError, KeyError, TypeError, zipfile.BadZipFile, subprocess.CalledProcessError) as error:
        print("image qualification: " + str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
