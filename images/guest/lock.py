"""Validate operator-selected, local guest inputs before any collection or boot."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import sys

GuestLock = dict[str, object]
VERSIONS = json.loads(Path(__file__).with_name("versions.json").read_text())
ROLES = {"kind-node", "controller", "ai-worker", "general-worker", "publisher", "session-loader", "vekil", "telegram", "teams"}
HEX = r"[0-9a-f]{64}"
DIGEST = r"sha256:" + HEX
NAME = r"[a-z0-9][a-z0-9+.-]{0,95}"
REPOSITORY = r"(?:localhost|[a-z0-9.-]+\.[a-z0-9.-]+|[a-z0-9.-]+:[0-9]+)/[a-z0-9]+(?:[._/-][a-z0-9]+)*"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def shape(value, required, optional=()):
    require(isinstance(value, dict), "expected object")
    require(set(required) <= value.keys() <= set(required) | set(optional), "missing or unknown fields")


def matches(value, pattern):
    return isinstance(value, str) and re.fullmatch(pattern, value) is not None


def json_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate JSON key")
        result[key] = value
    return result


def read_json(data):
    try:
        return json.loads(data, object_pairs_hook=json_object)
    except RecursionError as error:
        raise ValueError("JSON nesting exceeds decoder limit") from error


def load_lock(path: Path) -> GuestLock:
    value = read_json(path.read_bytes())
    validate_lock(value, production=False)
    return value


def relative_path(value):
    require(isinstance(value, str) and value and "\\" not in value, "invalid input path")
    path = PurePosixPath(value)
    require(not path.is_absolute() and all(p not in ("", ".", "..") for p in value.split("/")), "invalid input path")
    return path


def locked_file(root: Path, value: str) -> Path:
    relative = relative_path(value)
    root = root.resolve(strict=True)
    path = root
    for part in relative.parts:
        path = path / part
        require(not path.is_symlink(), "input symlink is not allowed")
    require(path.is_file() and path.resolve().is_relative_to(root), "missing or escaped input file")
    return path


def file_hash(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def checked_file(root, path, digest):
    require(matches(digest, HEX), "invalid file checksum")
    result = locked_file(root, path)
    require(file_hash(result) == digest, "input checksum mismatch: " + path)
    return result


def artifact_list(value):
    require(isinstance(value, list) and value, "missing artifacts")
    names = set()
    for item in value:
        shape(item, ("name", "version", "path", "sha256"))
        require(matches(item["name"], NAME) and item["name"] not in names, "invalid or duplicate artifact name")
        names.add(item["name"])
        version = item["version"]
        require(isinstance(version, str) and version and version.lower() not in {"latest", "main", "master", "head", "dev"}, "moving artifact version")
        require(matches(item["sha256"], HEX), "invalid artifact checksum")
        relative_path(item["path"])


def input_identity(lock: dict) -> str:
    """Identity policy excludes storage paths/tar headers, not selected software."""
    identity = {key: lock[key] for key in ("architecture", "sourceCommit", "nodeLocalRef", "kmxProfileSha256")}
    identity["orka"] = {key: lock["orka"][key] for key in ("mode", "identity", "chartSha256")}
    identity["components"] = sorted(
        [{key: item.get(key) for key in ("name", "repository", "manifestDigest", "configDigest", "indexDigest")} for item in lock["components"]],
        key=lambda item: item["name"],
    )
    for group in ("packages", "tools"):
        identity[group] = sorted(
            [{key: item[key] for key in ("name", "version", "sha256")} for item in lock[group]],
            key=lambda item: item["name"],
        )
    data = json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(data).hexdigest()


def validate_lock(lock: dict, *, production: bool) -> None:
    shape(lock, ("schemaVersion", "architecture", "sourceCommit", "orka", "components", "packages", "tools", "nodeLocalRef", "kmxProfileSha256"))
    require(lock["schemaVersion"] == "bridge-guest-inputs/v1", "unsupported input schema")
    require(lock["architecture"] in ("amd64", "arm64"), "unsupported architecture")
    require(matches(lock["sourceCommit"], r"[0-9a-f]{40}"), "invalid source commit")
    orka = lock["orka"]
    shape(orka, ("mode", "identity", "chartPath", "chartSha256"))
    require(matches(orka["chartSha256"], HEX), "invalid chart checksum")
    relative_path(orka["chartPath"])
    profile = lock["kmxProfileSha256"]
    require(profile is None or matches(profile, HEX), "invalid kmx profile checksum")
    if orka["mode"] == "source":
        require(not production, "production requires an Orka release including #833")
        require(lock["sourceCommit"] == orka["identity"] == VERSIONS["orkaSource"], "source identity mismatch")
    else:
        require(orka["mode"] == "release" and isinstance(orka["identity"], str), "unsupported Orka mode")
        approved = VERSIONS["releaseProfiles"].get(orka["identity"])
        # A bundle's own assertion that it includes #833 is not an approval.
        require(approved is not None, "no reviewed release profile including #833")
        require(profile is not None, "release requires kmx profile")
    components = lock["components"]
    require(isinstance(components, list), "missing components")
    names = set()
    for item in components:
        shape(item, ("name", "repository", "manifestDigest", "configDigest", "archive", "archiveSha256"), ("indexDigest", "indexPath"))
        require(item["name"] in ROLES and item["name"] not in names, "invalid or duplicate component")
        names.add(item["name"])
        require(matches(item["repository"], REPOSITORY), "repository must be explicit and untagged")
        for field in ("manifestDigest", "configDigest"):
            require(matches(item[field], DIGEST), "invalid component digest")
        require(matches(item["archiveSha256"], HEX), "invalid archive checksum")
        relative_path(item["archive"])
        require(("indexDigest" in item) == ("indexPath" in item), "incomplete release index proof")
        if "indexDigest" in item:
            require(matches(item["indexDigest"], DIGEST), "invalid index digest")
            relative_path(item["indexPath"])
    require(names == ROLES, "required component closure is incomplete")
    node = next(c for c in components if c["name"] == "kind-node")
    expected = "kindest/node:bridge-" + node["manifestDigest"][7:23] + "-" + lock["architecture"]
    require(lock["nodeLocalRef"] == expected, "node local tag must be derived from its platform digest")
    artifact_list(lock["packages"])
    artifact_list(lock["tools"])
    tools = {t["name"]: t for t in lock["tools"]}
    require("kind" in tools and tools["kind"]["version"] == VERSIONS["kind"], "kind version mismatch")
    bases = [p for p in lock["packages"] if p["name"] == "debian-base"]
    require(len(bases) == 1 and bases[0]["version"] == VERSIONS["debianBase"], "Debian base version mismatch")
    if orka["mode"] == "release":
        require(approved.get(lock["architecture"]) == input_identity(lock), "release profile input identity mismatch")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["validate"])
    parser.add_argument("--lock", type=Path, required=True)
    parser.add_argument("--mode", choices=["source", "release"], required=True)
    args = parser.parse_args()
    try:
        value = load_lock(args.lock)
        validate_lock(value, production=args.mode == "release")
    except (ValueError, OSError, TypeError) as error:
        print("guest lock: " + str(error), file=sys.stderr)
        return 2
    print("input identities valid; bytes and runtime remain unverified")
    return 0


if __name__ == "__main__":
    sys.exit(main())
