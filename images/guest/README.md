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

For source qualification, obtain the same-commit development descriptor selected
by `sourceOfflineProfile.artifactsSha256` in `versions.json`, and verify its
`sourceSHA` against `orkaSource` there. It records the controller, AI worker,
general-worker and publisher indexes for both architectures; select their
platform children and record the actual byte hashes. Restore the exact pinned
staged chart using [kmx's retained-chart extraction instructions](https://github.com/kaimahi-agents/kaimahi/blob/503b3ffcbdcc0a966e3b6de6235a8a98077b989e/docs/kmx.md#source-packaged-restricted-profile),
not a fresh Helm package or the older checked-in release chart. A local rebuild
instead requires approved **native builders for each architecture**; no builder
is provisioned here. Obtain the session loader, node,
Vekil, Telegram and Teams from their selected upstream sources; missing ARM64
artifacts cannot be omitted. Pin every base, package, tool, chart and archive by
its actual byte hash. The collector does not resolve tags, infer provenance from
labels, build source images, download dependencies or prove the asserted source
identity. Review the build receipts and selected lock before using it.

`input.schema.json` describes the lock's JSON shape. `lock.py` additionally checks
version policy, duplicate names/JSON keys, mandatory roles and cross-field rules.
Use one `bridge-guest-inputs/v2` lock per architecture for the native model route.
Legacy `bridge-guest-inputs/v1` locks retain their original nine-role closure.
The v2 closure adds an explicit `model-launcher`; it is not the session loader:

- `architecture`: `amd64` or `arm64`; `sourceCommit`: full Orka commit.
- `orka`: `mode` (`source` or `release`), `identity` (full commit or approved
  release), `chartPath` and unprefixed `chartSha256`.
- `components`: exactly `kind-node`, `controller`, `ai-worker`, `general-worker`,
  `publisher`, `session-loader`, `vekil`, `telegram`, `teams` and, for v2,
  `model-launcher`. Each has `name`,
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

For v2 source inputs, `lock.py` binds Vekil and the distinct BusyBox launcher to
platform manifests from the selected kmx `copilot-v1` profile. A directly pinned
platform manifest does not need an original multi-platform index. When supplied,
index evidence is still verified and retained. The model pins are keyed by kmx
source revision; the complete input identity binds both model roles. They are not
additional entries in kmx's unchanged core install manifest.

Offline assembly stages those two model archives alongside the six core roles.
Bootstrap imports them through the same containerd and exact-CRI checks, and
selects `--credential-mount copilot-v1` before kind first creates the node. Resume
uses kmx's existing mount validation, not a remount or directory-permission repair.
Vekil is not started by bootstrap: credential delivery followed by kmx Connect
remains a separate runtime operation. Telegram and Teams are still unstaged.
The public manifest records the v2 input identity and lock hash; it does not add
a separate launcher digest field or claim publication or credential qualification.

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
supply the endpoint's fixed utility paths and pre-created mount targets. The hook
puts the guest's util-linux and e2fsprogs executables at those paths, replacing
any klibc or busybox applet an earlier hook installed, and fails the build if it
cannot, so busybox is optional. The early hook accepts only the real serial
BootDescriptor; no test marker can authorize it.
The serve stage requires a fresh hello and existing-state reaffirmation for the
same lease. A failed early endpoint stops boot, including when shutdown fails.
This integration is not a production-ready laptop VM release.

## Export final boot assets

ARM64 assembly requires the exact kernel package and hashes in
`versions.json.arm64Kernel`. It retains the old installed kernel but selects
`console=hvc0`; AMD64 keeps its existing kernel selection and
`console=ttyS0,115200n8`. The selected endpoint must create credentials as
root:65532 mode 0750 on tmpfs with nosuid, nodev and noexec. The per-start guard
checks those properties and keeps the private runtime parent root:root mode 0700;
it never repairs a mount or creates a token. Token delivery remains a separate
integration with root:65532 mode 0640 at creation and replacement.

After assembly, export from the **final unbooted payload** and its `build.json`,
not AMD64's deleted raw intermediate or a test-instrumented disk:

```sh
python3 images/guest/boot_assets.py --lock /build/inputs/inputs.json \
  --image /build/new-os/os.qcow2 --build-receipt /build/new-os/build.json \
  --out /build/new-boot-assets
```

Use `os.raw` for ARM64. The lock additionally pins installed `unmkinitramfs`,
`readelf` and `dpkg-query` executables alongside `guestfish`. Their versions are
exact `--version` output, except unmkinitramfs uses its owning package version:
`dpkg-query -W -f='${Version}' initramfs-tools-core`. No tools are downloaded or
installed by the exporter. These pins cover direct tool invocations; helpers such
as cpio and decompressors and their shared libraries remain part of the retained
builder package/image provenance, not a hermetic-toolchain claim. It needs space for an owned copy of the final payload,
the selected module archive and expanded initrd. Guestfish opens that copy
read-only; guest executables are inspected as data, never run on the builder.

`os.kernel`, `os.initrd` and the last-written `boot-assets.json` bind the final
disk, build receipt, lock, extractor, inspection helper and extraction tools. The
receipt records the selected release, filesystem UUID root selector, supported
command line, kernel config, module tree, early endpoint and hook. It verifies
all five fixed initrd tools against canonical guest bytes and checks their
transitive ELF dependencies against the guest too. The endpoint, early hook and
five tools and ELF interpreters must have executable bits; shared-library comparisons do not require
them. ELF inspection uses LC_ALL=C without changing tool-version pin output.
The supported GRUB policy is the staged default 0 with one unconditional kernel
and matching initrd in the first menuentry. The optional `/vmlinuz` link must
agree when present. Quoted separators retain their context; unsupported commands,
executable conditions, backslash escapes and inline comments are refused rather
than evaluated. Stock module-loading conditions remain supported. Other or
indeterminate defaults are refused.
Required drivers must be
built in or present with the selected guest's module bytes. When initramfs-tools
expands a guest `.ko.xz` into `.ko`, comparison uses bounded XZ decompression;
the recorded module-tree identity still hashes the original stored files.
Unsupported kernel
formats, foreign bytes, mismatched GRUB kernel/initrd selections, saved GRUB
overrides and missing dependencies fail closed;
there is no automatic kernel decompression or fallback. A `cpio-prefix`
compression value describes a leading CPIO archive, potentially followed by a
compressed main archive, not an entirely uncompressed initrd.

The output directory must be new. A failed attempt can retain partial files but
has no completion receipt; keep it for diagnosis and use a different directory.
Exit 2 reports failure on stderr; success reports verification on stdout. All
qualification flags remain false. These files are build evidence, not the public
image manifest; publisher integration and native boot qualification follow
separately. Never replace the factory's exported initrd with one regenerated by
a disposable boot test.

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

The runner creates a private QCOW2 overlay over the immutable unbooted factory OS
and instruments only that overlay; the factory remains standalone.
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
The base case alone does not qualify application caches or Orka. Without the
native offline inputs below, OS assembly stages no application archives.
Source-fixture metadata cannot qualify Orka. Native platform boots and real-model
quality remain separate work. Every build and boot keeps `publicationReady: false`.

## Native offline infrastructure

To enable the native six-role profile, select additional hash-locked tools `kmx`,
`helm`, `kubectl`, `kmx-offline-manifest` and `orka-images`. The manifest is the
original supported `kmx-offline/v1` install.json, selected by hash, not another
Bridge schema. Its source, platform, complete profile, chart, descriptor, tool and
six image identities must agree with the input lock. Source bundles use the
`versions.json` pins: kmx `503b3ffcbdcc0a966e3b6de6235a8a98077b989e`, Orka
`3cb1f0df36705af9ff81cb8b4e8a0f8d36ee6eea` and profile
`orka-3cb1f0df-native/v1`. The profile selects restricted AI-worker RBAC and
controller context-token enforcement. This does not approve a production release.
Missing tools or references fail rather than download.

Assembly normalizes the six OCI archive references with the existing collector,
retaining original manifest/config/layer bytes. The node additionally becomes a
Docker-save archive, with original config and verified uncompressed layer diffIDs,
for Docker's classic outer store. The factory contains only the native node,
controller, AI worker, general-worker, publisher and session-loader payloads.
Future Vekil/adapter declarations are not shipped or qualified. Test archives,
model fixtures and initialized clusters are not part of the factory.

After validated state and both daemons, bridge-infrastructure.service runs the
stdlib-only lifecycle. It uses kmx exclusively for cluster creation and Orka
installation: `cluster up --step cluster --offline-bundle` and
`cluster orka install --provider - --offline-bundle`. There is no Helm/CRD shadow
installer. Cluster/context are installation-specific; kubeconfig and kmx state
live on the state disk. The minimal child environment contains no inherited
provider credentials. The lifecycle imports application OCI archives into the
owned node's k8s.io store and requires exact CRI references/config IDs before kmx
can install. API, CoreDNS, kmx rollout and PVC checks precede infrastructure-ready.
Rollout checks read Deployment status that survives a restart, so the lifecycle then
polls for 600 s until CoreDNS and the three Orka Deployments have pods the kubelet
reported Ready after the node container last started, and Orka's API answers
`/readyz` with status `ok` through its Service. Otherwise it refuses with the last
reason and the journal stays at orka-ready. On a slow first boot the unit's 900 s start
timeout can stop the run before that refusal; the journal still stays at orka-ready.
The unit does not retry, and the next run, such as the next boot, repeats every check.
A kmx refusal keeps leading context (including progress) and its closing lines in the lifecycle's error, without terminal escapes or its confirmation box. Long output keeps up to 40 leading lines and three closing lines with an omission marker, bounded to 8,192 characters; shorter output keeps all filtered lines. Other commands keep only their fixed name. The unit sends that error to the journal and the serial console.
Freshness compares kubelet and Docker timestamps, so it relies on the guest clock the
early endpoint sets from the host.
This marker is not authority: every resume rechecks ownership, caches and kmx.

The fsynced journal records identity-prepared, cluster-ready, cache-verified,
orka-ready and infrastructure-ready observations. A nonblocking per-state lock
serializes operations. Resume may start only exact journaled Docker IDs with the
selected node config and cluster label. Missing/replaced nodes, unjournaled nodes
and uncertain creation are retained and refused. A durable creation intent covers
the window before node IDs are journaled. Imports may safely repeat; failed or
pending Helm effects are left to kmx's refusal semantics, never forced or reset.

## Fixed infrastructure and Task reference cases

Run in the same externally isolated Linux KVM builder as the base proof:

```sh
python3 images/guest/boot_test.py --case infrastructure \
  --image /build/new-os/os.qcow2 --lock /build/inputs/inputs.json \
  --out /build/infrastructure-proof
python3 images/guest/boot_test.py --case restart \
  --image /build/new-os/os.qcow2 --lock /build/inputs/inputs.json \
  --out /build/restart-proof
python3 images/guest/boot_test.py --case interrupted \
  --image /build/new-os/os.qcow2 --lock /build/inputs/inputs.json \
  --out /build/interrupted-proof
python3 images/guest/tests/make_fixture.py --out /build/test-fixture
python3 images/guest/boot_test.py --case offline-tasks \
  --image /build/new-os/os.qcow2 --lock /build/inputs/inputs.json \
  --fixture /build/test-fixture/fixture.tar --out /build/task-proof
```

The fixture builder needs Go 1.26.2 and Git on the builder, not in the guest product.
It creates a separate digest-bound static deterministic Responses server and bare
local Git repository; the seed accepts no arbitrary command. Fixture state and
resources are exclusive to the disposable test cluster, with an explicit fixture
ServiceAccount. No real model credentials or external Git service are required.

Each boot that reaches infrastructure-ready checks once, without retrying, that Orka's
API answers `/readyz` through its Service, and records `orkaServing`.
Infrastructure checks real rendered worker/session-loader image arguments and no
initial Provider. It also boots a distinct new state B, attaches stopped A read-only,
and tries A's client identity against B with B's server/CA. B must first be reachable
with its own identity, then reject A for an attributable authorization/TLS reason.
Transient test credentials stay in private tmpfs, not the seed, manifest, argv or log.
Restart uses the same stopped state and EFI across an OS-only replacement and checks
node, namespace, snapshot secret/data, cluster CA, service-account key and etcd CA
fingerprints. These are identity checks, not a full etcd durability certification.
Interrupted setup stops after a real first import, cold-resumes the journal, then
injects a failed Helm release status into the owned test target. kmx must refuse it
without replacing the snapshot identity. The failed target remains failed; this is
not a claim that arbitrary partial installs are automatically repairable.

Offline Tasks require successful standalone and session-bound native AI Tasks,
a completed fetch-session init, and local-Git prepare-workspace using general-worker.
Evidence follows Task→Job→Pod UIDs and actual successful imageIDs, not only declared
images. It establishes deterministic fixture execution, not real-model quality.
Native cases retain NIC captures, zero observed external IPv4 TCP and DNS query names,
with the IPv4 counter excluding only traffic addressed back to the guest, not LAN
or other private destinations. IPv6 and VLAN traffic are not decoded by this counter;
with restricted QEMU user networking. Keep the builder externally isolated too.

Optional `--from-receipt /build/prior/boot.json` reuses that receipt's stopped `new`
state and EFI; selected software and disk contracts must match. The interrupted
case rejects this option before creating output or starting a VM. It is an explicit
owned-test capability, not a production migration path. It can mutate the prior
private test state. Never run it against a running or shared disk. All failure
artifacts are retained; choose a fresh output and never silently reset an ambiguous
target. Native-case timings are launch-to-poweroff, not isolated first-Task latency.
