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
