"""Verify a built guest ELF and record build-only provenance, never release approval."""
import argparse
import hashlib
import json
import pathlib
import struct
import subprocess

ROOT=pathlib.Path(__file__).resolve().parent.parent
TARGETS={"x86_64-unknown-linux-musl":62,"aarch64-unknown-linux-musl":183}

def inspect(header,report,target):
    if (len(header)<64 or header[:6]!=b"\x7fELF\x02\x01"
            or struct.unpack_from("<H",header,18)[0]!=TARGETS[target]
            or "INTERP" in report or "(NEEDED)" in report):
        raise ValueError("expected matching static ELF64 without interpreter or shared dependencies")
    return TARGETS[target]

def command(*args):
    return subprocess.check_output(args,cwd=ROOT,text=True,timeout=60).strip()

def digest(path):
    with open(path,"rb") as stream:
        return hashlib.file_digest(stream,"sha256").hexdigest()

def record(binary,target):
    report=command("readelf","-l","-d",str(binary))
    with binary.open("rb") as stream:
        machine=inspect(stream.read(64),report,target)
    if command("git","status","--porcelain"):
        raise ValueError("provenance requires a clean committed source checkout")
    compiler=pathlib.Path(command("rustup","which","--toolchain","1.98.1","rustc"))
    return {"schemaVersion":"bridge-guest-binary/v1","publicationReady":False,
            "qualification":"build-only; no guest boot",
            "sourceCommit":command("git","rev-parse","HEAD"),"target":target,
            "toolchain":"1.98.1","rustc":command("rustc","+1.98.1","-vV"),
            "rustcBinarySha256":digest(compiler),
            "cargoLockSha256":digest(ROOT/"runtime/Cargo.lock"),
            "protocolMajor":1,"contractVersion":"H1/v1",
            "binarySha256":digest(binary),"binaryBytes":binary.stat().st_size,
            "elf":{"machine":machine,"static":True}}

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target",choices=TARGETS,required=True)
    parser.add_argument("--binary",type=pathlib.Path,required=True)
    parser.add_argument("--out",type=pathlib.Path,required=True)
    args=parser.parse_args()
    result=record(args.binary.resolve(),args.target)
    args.out.parent.mkdir(parents=True,exist_ok=True)
    with args.out.open("x") as stream:
        json.dump(result,stream,indent=2,sort_keys=True);stream.write("\n")

if __name__=="__main__":
    main()
