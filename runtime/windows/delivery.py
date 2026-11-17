"""Acquire reviewed QEMU draft inputs and assemble a portable app; never publish."""
import argparse
import hashlib
import json
import re
import shutil
import subprocess
import tempfile
import zipfile
from pathlib import Path

import verify

HERE = Path(__file__).resolve().parent
SOURCES, PACKAGES = HERE / "sources.lock.json", HERE / "packages.lock.json"
NATIVE_SOURCE = HERE.parents[1] / "app/src-tauri/src/local_vm_native.rs"
REPOSITORY = "orka-agents/orka-bridge"
APP = "bridge-windows-x64.zip"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def input_tag(p):
    pair = ":".join(p["assets"][name]["sha256"] for name in verify.ARCHIVES)
    return "qemu-inputs-" + hashlib.sha256(pair.encode()).hexdigest()


def load_policy():
    p = json.loads((HERE / "runtime-inputs.json").read_bytes())
    require(p["repository"] == REPOSITORY and p["tag"] == input_tag(p), "unapproved input policy")
    return p


def validate_tag(tag):
    require(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", tag) is not None
            and ".." not in tag and not tag.endswith((".", ".lock")), "invalid app release tag")


def gh(endpoint, *, paginate=False, download=None):
    args = ["gh", "api", endpoint, "--method", "GET", "-H", "X-GitHub-Api-Version: 2022-11-28"]
    if paginate:
        args += ["--paginate", "--slurp"]
    if download:
        args += ["-H", "Accept: application/octet-stream"]
        with download.open("xb") as stream:
            subprocess.run(args, stdout=stream, check=True)
        return None
    return json.loads(subprocess.run(args, stdout=subprocess.PIPE, check=True).stdout)


def resolve_inputs(p):
    base = "repos/" + p["repository"]
    releases = [r for page in gh(base + "/releases?per_page=100", paginate=True)
                for r in page if r["tag_name"] == p["tag"]]
    require(len(releases) == 1, "expected exactly one QEMU input draft")
    r = releases[0]
    require(r["id"] == p["releaseId"] and r["draft"] is True and r["published_at"] is None
            and r["target_commitish"] == p["producerCommit"], "QEMU input draft identity mismatch")
    refs = [r for page in gh(base + "/git/matching-refs/tags/" + p["tag"], paginate=True)
            for r in page if r["ref"] == "refs/tags/" + p["tag"]]
    require(all(r["object"]["type"] == "commit" and r["object"]["sha"] == p["producerCommit"]
                for r in refs), "QEMU tag producer mismatch")
    assets = [a for page in gh(base + "/releases/" + str(p["releaseId"]) + "/assets?per_page=100",
                              paginate=True) for a in page]
    require(len(assets) == len(p["assets"]) and {a["name"] for a in assets} == set(p["assets"]),
            "QEMU input inventory mismatch")
    require(len({a["id"] for a in assets}) == len(assets), "duplicate QEMU asset identity")
    for a in assets:
        require(type(a["id"]) is int and a["id"] > 0 and a["state"] == "uploaded"
                and a["size"] == p["assets"][a["name"]]["bytes"], "QEMU asset size/state mismatch")
    return {a["name"]: {k: a[k] for k in ("id", "name", "size", "state")} for a in assets}


def check_file(path, row):
    require(path.is_file() and not path.is_symlink() and path.stat().st_size == row["bytes"]
            and verify.file_hash(path) == row["sha256"], "input hash/size mismatch: " + path.name)


def check_inputs(inputs, policy=None):
    p = policy or load_policy()
    require({x.name for x in inputs.iterdir()} == set(p["assets"]), "local QEMU inventory mismatch")
    for name, row in p["assets"].items():
        check_file(inputs / name, row)
    result = verify.verify(inputs, SOURCES, PACKAGES)
    with zipfile.ZipFile(inputs / verify.ARCHIVES[0]) as z:
        data = z.read("provenance/runtime-manifest.json")
    require(len(data) == p["runtimeManifest"]["bytes"]
            and hashlib.sha256(data).hexdigest() == p["runtimeManifest"]["sha256"], "runtime manifest pin mismatch")
    # The app owns this pin. A policy edit must not silently package another runtime.
    source = NATIVE_SOURCE.read_text(encoding="utf-8")
    pin = re.search(r'fn runtime_manifest_pin\(\).*?bytes:\s*(\d+),\s*sha256:\s*"([0-9a-f]{64})"', source, re.DOTALL)
    require(pin is not None and int(pin[1]) == len(data) and pin[2] == hashlib.sha256(data).hexdigest(),
            "compiled native runtime pin differs")
    return result


def fetch(out, policy=None):
    p = policy or load_policy()
    if out.exists():
        raise FileExistsError(out)
    run = gh("repos/" + p["repository"] + "/actions/runs/" + str(p["runId"]))
    require(run["id"] == p["runId"] and run["workflow_id"] == p["workflowId"]
            and run["head_sha"] == p["producerCommit"] and run["status"] == "completed"
            and run["conclusion"] == "success" and run["event"] == "workflow_dispatch"
            and run["repository"]["id"] == run["head_repository"]["id"] == p["repositoryId"],
            "unqualified QEMU producer run")
    assets = resolve_inputs(p)
    out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="qemu-inputs-", dir=out.parent) as tmp:
        stage = Path(tmp) / "inputs"
        stage.mkdir()
        for name, row in p["assets"].items():
            gh("repos/" + p["repository"] + "/releases/assets/" + str(assets[name]["id"]),
               download=stage / name)
            check_file(stage / name, row)
        require(resolve_inputs(p) == assets, "QEMU input draft changed during download")
        check_inputs(stage, p)
        stage.rename(out)


def assemble(inputs, executable, licenses, release_tag, out, policy=None):
    validate_tag(release_tag)
    if out.exists():
        raise FileExistsError(out)
    p = policy or load_policy()
    check_inputs(inputs, p)
    require(executable.is_file() and not executable.is_symlink(), "missing app executable")
    notices = sorted(licenses.glob("*.txt"))
    require(notices and all(n.is_file() and not n.is_symlink() for n in notices), "missing app notices")
    source = verify.ARCHIVES[1]
    notice = ("QEMU corresponding source: " + source + "\nSHA256: " + p["assets"][source]["sha256"]
              + "\nhttps://github.com/" + REPOSITORY + "/releases/download/" + release_tag + "/" + source
              + "\nOn a draft release, download this separate ZIP from that draft's asset list.\n"
                "Keep the runtime and its notices intact. Source is not downloaded at startup.\n")
    out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="windows-app-", dir=out.parent) as tmp:
        stage = Path(tmp) / "delivery"
        stage.mkdir()
        with zipfile.ZipFile(stage / APP, "w", compression=zipfile.ZIP_DEFLATED) as app:
            app.write(executable, "orka-bridge.exe")
            for n in notices:
                app.write(n, "licenses/" + n.name)
            app.writestr("QEMU-SOURCE.txt", notice)
            with zipfile.ZipFile(inputs / verify.ARCHIVES[0]) as runtime:
                for info in runtime.infolist():
                    # verify.verify already checked member paths/types/inventory.
                    with runtime.open(info) as src, app.open("native/qemu/" + info.filename, "w") as dst:
                        shutil.copyfileobj(src, dst, 1024**2)
        shutil.copyfile(inputs / source, stage / source)
        check_file(stage / source, p["assets"][source])
        stage.rename(out)
    return {"appBytes": (out / APP).stat().st_size, "appSha256": verify.file_hash(out / APP),
            "sourceBytes": (out / source).stat().st_size, "sourceSha256": verify.file_hash(out / source)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    fetch_parser = commands.add_parser("fetch")
    fetch_parser.add_argument("--out", type=Path, required=True)
    check = commands.add_parser("check")
    check.add_argument("--inputs", type=Path, required=True)
    build = commands.add_parser("assemble")
    for name in ("inputs", "executable", "licenses", "out"):
        build.add_argument("--" + name, type=Path, required=True)
    build.add_argument("--release-tag", required=True)
    args = parser.parse_args()
    if args.command == "fetch":
        fetch(args.out)
    elif args.command == "check":
        print(json.dumps(check_inputs(args.inputs), sort_keys=True))
    else:
        print(json.dumps(assemble(args.inputs, args.executable, args.licenses, args.release_tag, args.out), sort_keys=True))


if __name__ == "__main__":
    main()
