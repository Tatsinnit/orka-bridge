# Private Apple helper

The standalone `BridgeVM` executable composes the lifecycle core, native driver,
inherited capabilities, supervisor socket and helper-only TERM observer on one
serial queue. It is bundled but is not registered or launched by the app. No
Setup, Run or credential delivery is connected here. The private Rust session
composes the serving handshake for trusted native callers only.

## Build and resources

On Apple silicon with the Xcode Swift compiler installed:

```sh
bash scripts/build-vm-helper-macos.sh
bash scripts/test-vm-helper-macos.sh
```

The build script compiles Swift 5 for macOS 13, signs ad hoc with only
`com.apple.security.virtualization`, verifies the signature and publishes
`app/src-tauri/gen/apple-helper/BridgeVM`. Mac Cargo builds, tests, clippy and
Tauri development also build it before Tauri copies resources. Nothing installs
a compiler automatically. Every Mac shell build includes the fixed arm64 helper,
including Intel shell builds; this does not add an Intel VM backend. Generation
can cross-compile from either Mac host CPU; the executable test runners require
Apple silicon. Other target platforms do not invoke Swift.

The app bundle includes `Contents/Resources/native/BridgeVM`. The trusted
native backend resolves `app.path().resource_dir()?.join("native/BridgeVM")`;
there is no environment, renderer or downloaded executable selector. This is
development packaging, not Developer-ID signing or notarization qualification.
CI checks the real built and extracted resource without launching the helper:

```sh
bash scripts/check-vm-helper-bundle-macos.sh path/to/Bridge.app
```

## Private native input

Arguments must start with `--config-fd N`, followed by the unchanged three
pairs `--supervisor-fd N --guest-fd N --ownership-fd N` (those pairs may be
reordered). The fourth selector is stripped before `InheritedDescriptors.parse`.
The config FD must be a readable regular file distinct from the three inherited
capabilities. Parsing borrows all descriptors; successful parsing permits adoption.
The reader closes the adopted config FD, reads at offset zero and caps input at
64 KiB. No pathname arguments, credential input or new renderer IPC is provided.

The exact JSON fields are `version` (1), `installationPath`, `osDiskPath`,
`stateDiskPath` (absolute strings), `machineId` (1–4096 byte integers holding the
SDK representation), `mac` (six byte integers, locally administered unicast) and
`createEFI` (boolean). Unknown fields and invalid representations are rejected.
Only fixed exit status reaches diagnostics; errors never print SDK messages,
paths or identity bytes. The separate guest diagnostic serial writes to
`/dev/null`; it is not forwarded into app logs or the supervisor channel.

The producer uses the existing record and verified `LaunchSpec` under current
installation ownership. It uses the installed guest-mutable OS directly and
resizes the journal's existing state inode in place to 32 GiB. Immutable boot
files retain the merged image preflight; the initial OS hash is provenance.
The mode-0600, CLOEXEC config file is unlinked immediately before bounded JSON
writing and rewind. Guarded spawn transfers only the four selected capabilities
without changing parent CLOEXEC flags or inheriting ambient credentials.

The exact single argument `--machine-identifier` returns 1–4096 raw SDK identity
bytes on stdout without constructing a VM or writing installation files.
The native stdio-only initializer bounds output, enforces an absolute deadline
and reaps its child. This is future setup input before `Journal::create`;
reopen always uses the recorded identity, not a fresh candidate.

The backend retains one helper owner and the same installation capability after
stop or poison. EFI creation permission belongs only to its first prepared
launch and is consumed before launch; subsequent starts require the retained
store. The session delegates Early/New and Serve/Existing to `RuntimeWorker`.
It publishes the existing engine transport only after correlated Ready. Stop
unpublishes and seals guest I/O, observes native exit, then revokes the one socket
and awaits scoped references before returning success. Uncertain cleanup retains
ownership. This is not registry selection, setup readiness or credential delivery.

## Devices and lifetime

Configuration uses UEFI, four CPUs, trusted-config RAM (6 GiB), writable virtio disks named
`bridge-os` and `bridge-state`, entropy, NAT with the recorded MAC, the nonconsole
port zero `ai.orka.bridge.control` and a separate diagnostic serial. No shared
host folder or direct-Linux fallback exists. The only new installation file is
`efi.vars`: explicit creation refuses an existing path; reopen requires an
existing regular, non-symlink store. A partial or invalid store remains retained,
without replacement or automatic repair. Recovery is a later caller decision.
Path checks do not freeze other host writers; the caller must retain ownership
and the existing installed-image preflight.

Both writable raw images explicitly use `.cached` host caching and `.full`
synchronization. Full synchronization applies when the guest flushes; it is not
a replacement for graceful guest shutdown. The initializer and policy properties
are available on macOS 12+, below the helper's macOS 13 deployment target.

The socket and TERM observer exist before ownership transfers to `BridgeVM`.
`whenRegistered` gates start on actual kernel signal registration, not just source
resumption. Failed construction disposes an unstarted driver and closes its guest
handles before ownership. On terminal observation, the driver releases native
references and guest handles before the core closes ownership. Exit waits for
socket cancellation close and TERM disposition restoration; only completed
successful supervisor Stop plus both closes permits zero status. A Stop ACK is
not a guest clean-shutdown acknowledgment. Unobserved teardown retains ownership
until terminal observation or the parent's bounded process kill.

Tests exercise actual configuration objects and production composition with fake
VM operations, real sockets and signals; SDK-using test executables are signed.
They do not construct a valid native VM, attach a real installed image or boot a
guest. **Actual VZ attachment and callback/resource-release validation must pass
before backend activation.** Host sleep/death reason events still require the
later protocol revision. Compilation and bundle checks do not establish guest
boot, Orka readiness, NAT operation or production credential containment.
