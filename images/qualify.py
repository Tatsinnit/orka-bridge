"""Qualify pinned build bytes and create draft-only image assets; never publish."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from images.guest.lock import (HEX, VERSIONS, file_hash, input_identity, matches,
                               read_json, relative_path, require, shape, validate_lock)

REPOSITORY = "orka-agents/orka-bridge"
SLUG = r"[a-z0-9][a-z0-9.-]{0,62}"


def gh(endpoint, *, paginate=False, download=None, body=None, upload=None):
    """Only network boundary: official gh authentication, never credentials in argv."""
    args = ["gh", "api", endpoint, "--method", "POST" if body is not None or upload else "GET",
            "-H", "X-GitHub-Api-Version: 2022-11-28"]
    if paginate:
        args += ["--paginate", "--slurp"]
    if download:
        args += ["-H", "Accept: application/octet-stream"]
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


def approved_profile(name):
    policy = read_json((ROOT / "images/publisher-policy.json").read_bytes())
    shape(policy, ("schemaVersion", "acceptedSchemaSha256", "profiles"))
    require(policy["schemaVersion"] == "bridge-image-policy/v1", "unsupported publisher policy")
    require(isinstance(policy["profiles"], dict), "invalid profiles")
    require(matches(policy["acceptedSchemaSha256"], HEX) and name in policy["profiles"],
            "schema/profile not approved; no network permitted")
    require(file_hash(ROOT / "images/manifest.schema.json") == policy["acceptedSchemaSha256"],
            "schema bytes not approved")
    p = policy["profiles"][name]
    shape(p, ("repository", "repositoryId", "runId", "workflowId", "sourceCommit",
              "artifactId", "artifactSha256", "payloadPath", "payloadSha256",
              "metadataPath", "metadataSha256", "imageId", "platform", "architecture",
              "nativeReceiptSha256", "inputIdentity"))
    require(p["repository"] == REPOSITORY, "unapproved repository")
    for field in ("repositoryId", "runId", "workflowId", "artifactId"):
        require(type(p[field]) is int and p[field] > 0, "invalid API identity")
    for field in ("artifactSha256", "payloadSha256", "metadataSha256",
                  "nativeReceiptSha256", "inputIdentity"):
        require(matches(p[field], HEX), "invalid profile checksum")
    require(matches(p["sourceCommit"], r"[0-9a-f]{40}") and matches(p["imageId"], SLUG) and ".." not in p["imageId"],
            "invalid source/image identity")
    require((p["platform"], p["architecture"]) in (("macos", "arm64"), ("windows", "amd64")),
            "unsupported profile target")
    for field in ("payloadPath", "metadataPath"):
        relative_path(p[field])
    require(p["payloadPath"] != p["metadataPath"], "selected members must differ")
    require(bool(VERSIONS["releaseProfiles"]), "no reviewed guest production profiles")
    return p


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
    require(artifact["id"] == p["artifactId"] and artifact["expired"] is False
            and artifact["digest"] == "sha256:" + p["artifactSha256"], "artifact mismatch")
    source = artifact["workflow_run"]
    require(source["id"] == p["runId"]
            and source["repository_id"] == source["head_repository_id"] == p["repositoryId"]
            and source["head_sha"] == p["sourceCommit"], "artifact source mismatch")


def archive_metadata(p, archive):
    names = set()
    for entry in archive.infolist():
        name = entry.filename.rstrip("/") if entry.is_dir() else entry.filename
        relative_path(name)
        mode = stat.S_IFMT(entry.external_attr >> 16)
        require(name not in names and mode in (0, stat.S_IFREG, stat.S_IFDIR),
                "duplicate or nonregular ZIP member")
        names.add(name)
    for field in ("payloadPath", "metadataPath"):
        require(p[field] in names and not archive.getinfo(p[field]).is_dir(), "missing selected ZIP member")
    info = archive.getinfo(p["metadataPath"])
    require(info.file_size <= 8 * 1024 * 1024, "metadata too large")
    with archive.open(info) as stream:
        raw = stream.read(8 * 1024 * 1024 + 1)
    require(len(raw) <= 8 * 1024 * 1024
            and hashlib.sha256(raw).hexdigest() == p["metadataSha256"], "metadata checksum mismatch")
    with archive.open(p["payloadPath"]) as stream:
        require(hashlib.file_digest(stream, "sha256").hexdigest() == p["payloadSha256"],
                "payload checksum mismatch")
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
        gh("repos/" + p["repository"] + "/actions/artifacts/" + str(p["artifactId"]) + "/zip", download=path)
        require(file_hash(path) == p["artifactSha256"], "downloaded ZIP checksum mismatch")
        with zipfile.ZipFile(path) as archive:
            metadata = archive_metadata(p, archive)
            bind_metadata(p, metadata)
            validate_lock(metadata["componentLock"], production=True)
            with archive.open(p["payloadPath"]) as stream:
                package.create(stream, metadata, out)
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
    files = [c["name"] for c in manifest["artifact"]["chunks"]] + ["manifest.json"]
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
