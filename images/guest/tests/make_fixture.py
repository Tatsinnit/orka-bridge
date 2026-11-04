#!/usr/bin/python3
"""Build the separate deterministic AMD64 test seed (never a factory input)."""
import argparse
import hashlib
import io
import json
import os
import subprocess
import tarfile
from pathlib import Path

SOURCE = Path(__file__).resolve().parent / "fixtures/model-server.go"


def command(args, **kw):
    return subprocess.run(args, check=True, capture_output=True, **kw).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    root = args.out.resolve()
    root.mkdir()
    if command(["go", "version"]).decode().split()[2] != "go1.26.2":
        raise RuntimeError("requires pinned Go 1.26.2")
    work = root / "build"
    work.mkdir()
    env = dict(os.environ, CGO_ENABLED="0", GOOS="linux", GOARCH="amd64", GOAMD64="v1",
               GOFLAGS="", GOWORK="off", GOENV="off")
    command(["go", "build", "-trimpath", "-buildvcs=false", "-ldflags=-s -w -buildid=",
             "-o", str(work / "model-server"), str(SOURCE)], env=env)
    repo = work / "repo"
    if not repo.exists():
        command(["git", "init", "--bare", "--object-format=sha1", "--initial-branch=main", str(repo)])
    git = ["git", "-c", "core.hooksPath=/dev/null", "--git-dir=" + str(repo)]
    blob = command([*git, "hash-object", "-w", "--stdin"], input=b"Offline fixture repository.\n").decode().strip()
    tree = command([*git, "mktree"], input=("100644 blob " + blob + "\tREADME.txt\n").encode()).decode().strip()
    commit_env = dict(os.environ, GIT_AUTHOR_NAME="Fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid",
                      GIT_COMMITTER_NAME="Fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid",
                      GIT_AUTHOR_DATE="2000-01-01T00:00:00Z", GIT_COMMITTER_DATE="2000-01-01T00:00:00Z")
    commit = command([*git, "commit-tree", tree], input=b"Fixture\n", env=commit_env).decode().strip()
    command([*git, "update-ref", "refs/heads/main", commit])
    command([*git, "update-server-info"])
    # Do not archive Git templates, hooks or mutable local configuration.
    names = ["HEAD", "config", "description", "info/refs", "objects/info/packs", "refs/heads/main"]
    names += [p.relative_to(repo).as_posix() for p in repo.glob("objects/[0-9a-f][0-9a-f]/*") if p.is_file()]
    entries = {"model-server": (work / "model-server").read_bytes()}
    for name in sorted(set(names)):
        if name == "config":
            data = b"[core]\n\trepositoryformatversion = 0\n\tbare = true\n"
        elif name == "description":
            data = b"Offline fixture\n"
        else:
            data = (repo / name).read_bytes()
        entries["repo/" + name] = data
    archive = root / "fixture.tar"
    with tarfile.open(archive, "w", format=tarfile.USTAR_FORMAT) as stream:
        for name, data in sorted(entries.items()):
            item = tarfile.TarInfo(name)
            item.mode = 0o755 if name == "model-server" else 0o644
            item.size = len(data)
            stream.addfile(item, io.BytesIO(data))
    if archive.stat().st_size > 32 << 20:
        raise RuntimeError("seed exceeds cap")
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (root / "fixture.tar.sha256").write_text(digest + "  fixture.tar\n")
    (root / "fixture.json").write_text(json.dumps({"schemaVersion": "bridge-task-fixture-build/v1", "architecture": "amd64", "sha256": digest, "fileBytes": archive.stat().st_size, "sourceSha256": hashlib.sha256(SOURCE.read_bytes()).hexdigest(), "compiler": command(["go", "version"]).decode().strip(), "publicationReady": False}, sort_keys=True) + "\n")
    print(digest, archive.stat().st_size)


if __name__ == "__main__":
    main()
