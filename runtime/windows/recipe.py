"""Validate selected source inputs, without downloading or executing them."""
import argparse
import hashlib
import json
import re
import stat
import sys
from pathlib import Path
from urllib.parse import urlsplit

PATCHES = ("0001", "0005", "0009", "0015", "0019", "0024")
CHECKS = {"test-socket-close.py", "test-fd-setsize.py", "test-whpx-cpu-model.py"}
GROUPS = ("patches", "checks", "wraps")
RESERVED = {"CON", "PRN", "AUX", "NUL", *(f"{p}{n}" for p in ("COM", "LPT") for n in range(1, 10))}


def _keys(value, keys):
    if not isinstance(value, dict) or set(value) != set(keys.split()):
        raise ValueError(f"expected fields: {keys}")


def _hex(value, length):
    if not isinstance(value, str) or not re.fullmatch(rf"[0-9a-f]{{{length}}}", value):
        raise ValueError(f"expected lowercase {length}-character digest or commit")


def _path(value):
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*", value):
        raise ValueError(f"unsafe source path: {value!r}")
    for part in value.split("/"):
        if part in (".", "..") or part.endswith(".") or part.split(".")[0].upper() in RESERVED:
            raise ValueError(f"unsafe source path: {value!r}")
    return value


def _repository(value):
    if not isinstance(value, str):
        raise TypeError("expected HTTPS source repository")
    url = urlsplit(value)
    if (url.scheme != "https" or url.netloc not in ("github.com", "gitlab.com")
            or not url.path.strip("/") or url.query or url.fragment):
        raise ValueError(f"invalid source repository: {value!r}")


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def load_sources(path):
    """Check the source-selection shape; commit existence is not proven here."""
    try:
        data = json.loads(Path(path).read_text(encoding="utf-8"), object_pairs_hook=_unique_object)
        _keys(data, "schemaVersion sourceDateEpoch reference qemu patches checks submodules wraps")
        if data["schemaVersion"] != "bridge-windows-sources/v1":
            raise ValueError("unsupported source lock schema")
        if type(data["sourceDateEpoch"]) is not int or data["sourceDateEpoch"] < 315532800:
            raise ValueError("invalid sourceDateEpoch")
        for name, keys in (("reference", "repository commit"), ("qemu", "repository commit baseCommit")):
            _keys(data[name], keys)
            _repository(data[name]["repository"])
            _hex(data[name]["commit"], 40)
        _hex(data["qemu"]["baseCommit"], 40)
        if (data["reference"]["repository"] != "https://github.com/omacom/try-omarchy-windows"
                or data["qemu"]["repository"] != "https://github.com/cmspam/winq-emu-qemu"):
            raise ValueError("unexpected source repository selection")
        origins = {
            "patches": f"https://raw.githubusercontent.com/omacom/try-omarchy-windows/{data['reference']['commit']}/runtime-build/",
            "checks": f"https://raw.githubusercontent.com/omacom/try-omarchy-windows/{data['reference']['commit']}/runtime-build/",
            "wraps": f"https://raw.githubusercontent.com/cmspam/winq-emu-qemu/{data['qemu']['commit']}/",
        }
        seen = set()
        for group in (*GROUPS, "submodules"):
            if not isinstance(data[group], list) or not data[group]:
                raise ValueError(f"missing source group: {group}")
            for item in data[group]:
                _keys(item, "path repository commit" if group == "submodules" else "path url bytes sha256")
                name = _path(item["path"])
                if name.casefold() in seen:
                    raise ValueError(f"duplicate source path: {name}")
                seen.add(name.casefold())
                if group == "submodules":
                    _repository(item["repository"])
                    _hex(item["commit"], 40)
                    continue
                if type(item["bytes"]) is not int or item["bytes"] <= 0:
                    raise ValueError(f"invalid byte length: {name}")
                _hex(item["sha256"], 64)
                relative = name.removeprefix("checks/") if group == "checks" else name
                if item["url"] != origins[group] + relative:
                    raise ValueError(f"source URL must match pinned origin and path: {name}")
                if group == "patches" and not (name.startswith("patches/qemu/") and name.endswith(".patch")):
                    raise ValueError(f"unexpected patch path: {name}")
                if group == "checks" and not name.startswith("checks/"):
                    raise ValueError(f"unexpected check path: {name}")
                if group == "wraps" and not (name.startswith("subprojects/") and name.endswith(".wrap")):
                    raise ValueError(f"unexpected wrap path: {name}")
        if tuple(Path(x["path"]).name[:4] for x in data["patches"]) != PATCHES:
            raise ValueError("patch selection or application order differs from headless profile")
        if sorted(Path(x["path"]).name for x in data["checks"]) != sorted(CHECKS):
            raise ValueError("required source checks are missing or substituted")
        return data
    except (OSError, UnicodeError, TypeError, ValueError) as error:
        raise ValueError(f"{path}: {error}") from error


def check_sources(lock, inputs):
    """Hash regular source files; inputs must remain stable for the whole check."""
    try:
        root = Path(inputs).resolve(strict=True)
        if not root.is_dir():
            raise ValueError("not a directory")
        count = 0
        for group in GROUPS:
            for item in lock[group]:
                path = root
                for part in _path(item["path"]).split("/"):
                    path = path / part
                    info = path.lstat()
                    if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
                        raise ValueError(f"link or reparse point: {item['path']}")
                if not stat.S_ISREG(info.st_mode):
                    raise ValueError(f"not a regular source file: {item['path']}")
                with path.open("rb") as stream:
                    size = 0
                    digest = hashlib.sha256()
                    while chunk := stream.read(1024 * 1024):
                        size += len(chunk)
                        if size > item["bytes"]:
                            raise ValueError(f"source byte length mismatch: {item['path']}")
                        digest.update(chunk)
                if size != item["bytes"] or digest.hexdigest() != item["sha256"]:
                    raise ValueError(f"source size/sha256 mismatch: {item['path']}")
                count += 1
        return count
    except (OSError, ValueError) as error:
        raise ValueError(f"source inputs {inputs}: {error}") from error


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    check = commands.add_parser("check-sources", help="validate selection and optionally staged source bytes")
    check.add_argument("--lock", required=True, type=Path)
    check.add_argument("--inputs", type=Path)
    args = parser.parse_args(argv)
    try:
        lock = load_sources(args.lock)
        count = check_sources(lock, args.inputs) if args.inputs is not None else 0
        result = {"schemaVersion": "bridge-windows-source-check/v1", "publicationReady": False,
                  "qualification": "source-selection-only; no build or patch application",
                  "inputBytesVerified": args.inputs is not None, "filesVerified": count,
                  "qemuCommit": lock["qemu"]["commit"]}
        print(json.dumps(result, sort_keys=True))
        return 0
    except ValueError as error:
        print(str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
