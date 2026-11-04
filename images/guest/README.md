# Guest inputs

This directory owns build-time Linux guest inputs, separate from the desktop app.
The local collector validates reviewed input locks and rewrites OCI root indexes
with exact image references. It does not create a VM, install Orka, log in to a
registry, run an agent Task, or publish anything.

## Check the collector

Python 3.11 or newer is required. There are no Python package dependencies.
From the repository root:

```sh
PYTHONPATH=images python3 -m unittest discover -s images/guest/tests -t images -v
python3 images/guest/lock.py --help
python3 images/guest/cache.py --help
actionlint .github/workflows/guest-checks.yml
```

Tests synthesize tiny **non-bootable** OCI archives in temporary directories.
They exercise byte integrity, original repository/digest references, selected
platform membership, deterministic output, bad paths, symlinks, duplicate entries,
missing components, source identity and unsupported production profiles. They do
not establish CRI import, worker usability, ARM64 execution or guest readiness.
CI runs these tests in a network namespace with no external interface.

## Select actual inputs

`versions.json` selects the dated Debian base, kind, Kubernetes and interim Orka
source commit. It is not a complete artifact lock. Production depends on the
**Orka release including #833**. No released profile is approved yet; production
validation deliberately fails until a reviewed profile with its chart and kmx
hashes is added to the checked-in version policy. Each approved release profile
maps architecture to `input_identity(lock)`: the hash binds every selected image
repository/manifest/config/index, chart, source, package, tool and kmx profile,
not caller-controlled paths or tar headers. A profile cannot authorize replacing
an individual worker image or executable. A bundle cannot self-assert
that it supports the release. Source qualification and production stay distinct.

For source qualification, use the existing same-commit development descriptor
from [the successful #833 build](https://github.com/orka-agents/orka/actions/runs/41533123834),
whose `sourceSHA` matches `versions.json`. It records the controller, AI worker,
general-worker and publisher indexes for both architectures; select their
platform children and record the actual byte hashes. Package that commit's
staged source chart, not the older checked-in release chart. A local rebuild
instead requires approved **native builders for each architecture**; no builder
is provisioned here. Obtain the session loader, node,
Vekil, Telegram and Teams from their selected upstream sources; missing ARM64
artifacts cannot be omitted. Pin every base, package, tool, chart and archive by
its actual byte hash. The collector does not resolve tags, infer provenance from
labels, build source images, download dependencies or prove the asserted source
identity. Review the build receipts and selected lock before using it.

`input.schema.json` describes the lock's JSON shape. `lock.py` additionally checks
version policy, duplicate names/JSON keys, mandatory roles and cross-field rules.
Use one `bridge-guest-inputs/v1` lock per architecture:

- `architecture`: `amd64` or `arm64`; `sourceCommit`: full Orka commit.
- `orka`: `mode` (`source` or `release`), `identity` (full commit or approved
  release), `chartPath` and unprefixed `chartSha256`.
- `components`: exactly `kind-node`, `controller`, `ai-worker`, `general-worker`,
  `publisher`, `session-loader`, `vekil`, `telegram` and `teams`. Each has `name`,
  explicit untagged `repository`, prefixed `manifestDigest` and `configDigest`,
  relative `archive` path and unprefixed `archiveSha256`.
- When the selected child came from a multi-platform release index, include
  `indexPath` and prefixed `indexDigest`. The index bytes must contain that
  child digest at `linux` and the selected architecture; a lock value alone
  is not membership evidence.
- `packages` and `tools`: nonempty arrays of `name`, exact `version`, relative
  `path`, and unprefixed `sha256`. Packages include the selected `debian-base`;
  tools include the selected `kind`. A Debian package dependency closure is
  selected by the operator and must later be exercised in offline assembly.
- `nodeLocalRef`: `kindest/node:bridge-<first 16 manifest hex digits>-<architecture>`.
- `kmxProfileSha256`: the reviewed profile's unprefixed hash, or `null` while
  source input work precedes offline-bundle support. Null is not a usable
  kmx install profile and cannot establish a complete candidate.

All file paths are relative to the lock's directory; absolute paths, `..`,
backslashes and symlinks are refused. Keep inputs stable during collection.
Do not put credentials, user state or private build-machine paths in locks.
Fixtures in `tests/fixtures.py` illustrate the shape but are not actual inputs.

Each input OCI tar must contain `oci-layout`, `index.json` selecting **one
platform manifest**, and exactly that manifest's config and layer blobs under
`blobs/sha256/`. OCI and Docker schema-2 image media types are accepted. For a
multi-platform source, supply a selected-child archive and the original index
proof separately; do not relabel an index digest as a platform digest. Tar links,
unsafe or duplicate names, unrelated files, missing blobs and blob size/hash
mismatches are refused. Files are read individually, never extracted.

## Collect

With a real lock and its actual files ready:

```sh
python3 images/guest/lock.py validate --lock /build/inputs/amd64.json --mode source
python3 images/guest/cache.py --lock /build/inputs/amd64.json --out /build/cache-amd64
```

The first command validates identities/structure, **not file bytes**. Collection
verifies every supplied file and OCI graph before creating a new output directory.
The output must not exist. Repeated inputs produce byte-identical archives and
receipts. If an I/O failure interrupts writing, the directory may be partial;
a successful collection requires its final `collection.json`, published by atomic
rename only after the receipt is fully written. A failed receipt write can leave
`collection.json.partial` but never the final marker. Choose a fresh
output directory rather than overwriting it.

Output contains:

- `inputs.json`: a normalized lock with local output paths, consumable by
  `load_lock` and subsequent assembly. The receipt binds its exact byte hash.
- `components/*.oci.tar`: original repository plus platform digest in
  `org.opencontainers.image.ref.name`. The node uses `nodeLocalRef`. Manifest,
  config and layer bytes remain unchanged; tar metadata is deterministic.
- `artifacts/<sha256>` and `chart.tgz`: verified packages, tools, optional
  original release indexes and chart. Tool executable modes are intentionally
  not installed here; later assembly owns installation permissions.
- `collection.json`: canonical input identity, normalized lock hash, output byte
  hashes, architecture, source/release and kmx profile identities. No host paths
  or time-dependent fields are copied. `publicationReady` is always false.

These are OCI archives, not Docker-save archives. Do not feed the node archive
directly to a Docker version that cannot load OCI layout; the later cluster
installer must consume/convert it locally without changing image identity.
The later guest path must import application archives in node containerd's
`k8s.io` namespace, prove exact CRI references, and pass native standalone and
session-bound Tasks with registry egress blocked. This collector does not claim
those gates passed. Hashes bind selected bytes, not upstream authenticity or
code signing. No image, chart, archive or release is published by these commands.

## Offline OS assembly

Use a native Linux builder with Python 3.11+, KVM, libguestfs, QEMU, OVMF and
`genisoimage`. These are **builder dependencies**, not requirements on the
creator's laptop. Collect the dated Debian factory disk and complete local Debian
package dependency set before assembly. Do not use a previously booted guest as
factory input. The selected profile requires one ext4 root, one EFI filesystem
and only optional unformatted BIOS space; other or extra filesystems are rejected,
not silently removed from fstab. The assembler rejects known installation directories rather than
attempting to erase a configured installation.

The existing lock's `packages` list contains `debian-base` plus the Debian packages
needed for Docker, containerd, jq, initramfs tools and their dependencies. Package
names, versions and exact file hashes are selected during input collection, not
resolved by the offline assembler. The `tools` list also pins `qemu-img`,
`guestfish`, `virt-resize` and `virt-customize`: `path` names a retained copy of
each executable, `sha256` must match the installed executable, and `version` is
its exact `--version` output. Also select `bridge-guest` and
`bridge-guest-provenance` tool files with the same source commit as their version.
The original `bridge-guest-binary/v1` record must bind the binary bytes/hash,
architecture target, protocol major and two-disk contract. Installation copies the
selected binary, original provenance and project notices; it does not rebuild it. The builder's kernel, shared libraries and execution
environment must also be retained in build provenance, such as a selected local
builder image digest. No downloader or package fallback runs during assembly.

```sh
LIBGUESTFS_BACKEND=direct python3 images/guest/build.py \
  --lock /build/inputs/inputs.json --out /build/new-os
```

Run the builder without an external network interface in addition to
`virt-customize --no-network`. Assembly uses libguestfs, local files and `dpkg`,
with service startup disabled. It installs an EFI fallback loader and a clean OS,
not a pre-created cluster. AMD64 output is standalone GPT QCOW2; ARM64 output is
GPT raw. The OS has 16 GiB virtual capacity. A separate descriptor requests a new
32 GiB state disk, not a distributed initialized filesystem. Actual file and
allocated sizes are recorded in `build.json` and differ from virtual capacity.

The initramfs hook selects state by `bridge-state` serial, validates its ext4 UUID
and persistent journal, binds machine identity before systemd, and mounts private
credential memory before either daemon. Docker and separate containerd have
state-root settings and per-start guards. Unknown state never becomes formatting
permission. Swap, hibernation, login gettys and SSH are disabled; the console is
for diagnostics. The control port is named `ai.orka.bridge.control` and root-only.

The same selected static endpoint runs `--early` in initramfs and `--serve` as a
normal service. Initramfs must use `/root`, retain `/run` across switch-root, and
supply the endpoint's fixed utility paths and pre-created mount targets. The early
hook accepts only the real serial BootDescriptor; no test marker can authorize it.
The serve stage requires a fresh hello and existing-state reaffirmation for the
same lease. A failed early endpoint stops boot, including when shutdown fails.
This integration is not a production-ready laptop VM release.

## Disposable reference boot

```sh
LIBGUESTFS_BACKEND=direct python3 images/guest/boot_test.py --case base \
  --image /build/new-os/os.qcow2 --lock /build/inputs/inputs.json \
  --out /build/new-reference-run
```

This runner is Linux AMD64 KVM only, using QEMU's `pc-q35-7.2` machine with matching
OVMF 4M firmware and variable template. It uses 4 vCPU and 6 GiB RAM. Keep the
runner isolated from external networking; user networking is also restricted.
It neither invokes native app backends nor publishes anything.

The runner copies the unbooted factory OS and instruments only that private copy.
A separate read-only seed uses `bridge-guest-test/v1` with case, installation UUID,
explicit `new` or `existing` authority and the state serial. Only the runner's
exclusively created empty state receives `new`. No arbitrary commands are accepted.
The observer, normal test service and verifier are absent from factory output.
The seed is test metadata only: it never populates the endpoint's accepted boot
record. The reference host exchanges the real early/serve frames, retains fresh
stage nonces and checks endpoint readiness. It requests shutdown only after both
endpoint readiness and the guest verifier have completed. No reference formatter
is installed.
The instrumented disk and seed hashes are separate from the factory hash.

The base case checks real daemon roots, mount types, early ordering, control-port
permissions and a persistent sentinel, then shuts down. It replaces only the OS,
reuses the same stopped state and EFI files, and checks the same machine identity.
Missing-state, mismatched-UUID and blank-existing boots must refuse; neither of
the latter two may change logical state contents. Logs and failed private disks are retained, never silently reset.

`boot.json` follows `qualification.schema.json`. It records the actual backend,
inputs, firmware, instrumentation and seed hashes, per-case disk bytes and elapsed
VM launch-to-poweroff time. That time is not cluster readiness or first Task time.
The OS check covers only selected OS inputs: application archives are neither
installed nor verified by this step. Source-fixture metadata cannot qualify Orka.
Offline cluster setup, native platform boots and a real model Task remain later
work. Both build and boot results keep `publicationReady: false`.
