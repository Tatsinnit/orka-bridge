# Windows runtime producer

This produces engineering archives of source-built, headless AMD64 QEMU for
Windows. It is not a release or an application download path. CI builds QEMU and
runs non-boot inspection commands; it never starts a guest. Windows application
integration, direct-boot images and native VM qualification are separate work.
Every verification result keeps `publicationReady: false`.

## Check the inputs locally

From the repository root, with Python 3.11 or later:

```sh
python -m unittest discover -s runtime/windows -p 'test_*.py' -v
python runtime/windows/recipe.py check-locks --sources runtime/windows/sources.lock.json --packages runtime/windows/packages.lock.json
```

These commands do not download or execute the locked inputs. The existing
`check-sources --lock runtime/windows/sources.lock.json [--inputs DIR]` command
still distinguishes metadata validation from actual staged-byte verification.
`check-inputs --sources FILE --packages FILE --inputs DIR` verifies all declared
archive/file bytes before bootstrap execution. Unrelated files are not input
selection; package installation uses the explicit target list, not a directory
glob. Inputs must remain stable throughout the build.

`sources.lock.json` pins the QEMU fork, upstream base, reference recipe, six
ordered reference patches, three retained checks, wraps and original top-level
gitlinks. One owned `disable-dsound.patch` makes detection respect a disabled
option without changing enabled or auto behavior. Its bytes and the resulting
Git tree are locked; it is included in the corresponding-source rebuild recipe.
`packages.lock.json` records the dated MSYS2 bootstrap inventory, all external
packages, exact final installed inventory, Python wheels, corresponding sources,
notices, firmware ownership, historical recipes and additional Git sources.
Git records and downloadable file records occupy separate lock groups. The
package lock is generated data, not a package-manager update instruction.
The installed-data map also retains QEMU's desktop entry, tracing definitions and
artwork. The original SVG's CC BY 3.0 attribution and license link are preserved
with the bitmap derivation recipe, not replaced by QEMU's GPL notice.

Archive sizes and SHA-256 values come from the publisher's repository metadata
or explicitly recorded inspection of immutable source bytes. Metadata snapshot
hashes and captured detached signatures are retained. Capturing a signature is
not a claim of cryptographic verification. The separately dated bootstrap and
repository snapshots are not an atomic repository transaction. CI must prove the
selected closure installs, not merely that the dependency records look complete.

## Build and inspect

Path-filtered checks validate source changes on pull requests and branch pushes.
The producer runs only when manually dispatched on `main`; pull requests and
branch pushes do not build or upload archives. Start it after the workflow change
has merged with `gh workflow run windows-runtime.yml --ref main`.
It checks out the exact selected main commit with read-only permissions and no
persisted checkout credentials. It verifies all
inputs, extracts the pinned bootstrap into a new runner-local directory, and
installs only locked local packages. Its temporary pacman configuration preserves
the bootstrap's options but contains no repositories: no database refresh or
online dependency resolution. The final installed inventory must match exactly.

`build.sh INPUTS NEW_OUTPUT` runs inside that fresh MSYS2 UCRT64 environment.
Archive acquisition belongs to PowerShell; the script fetches only pinned Git
sources before configuration. No package version or source revision floats.
Configure has downloads disabled and uses vendored Python wheels. Default guest
devices and installed firmware remain intact; host graphics, audio, USB
passthrough, plugins, TCG, Rust and unneeded default features are disabled.
WHPX, slirp, tools, zstd and pixman are required. Effective Meson options and
compiled configuration are checked; unknown states are not treated as disabled.

The producer copies the two executables and recursively inspects their actual
PE imports using the locked objdump. Every non-system DLL must resolve uniquely
inside the locked UCRT64 prefix and have a selected package/source/notice owner.
System DLLs and API sets are explicit and are never copied. It does not copy an
entire toolchain `bin` directory or rely on a filtered `ldd` result.

The output directory must not already exist. It contains:

- `bridge-windows-runtime-portable.zip`: console system emulator,
  `qemu-img`, reachable DLLs, unpruned QEMU data, notices and provenance.
- `bridge-windows-runtime-source.zip`: exact patched source, ROM sources and
  matching historical firmware dependencies, original patches/checks/wraps,
  official package source archives and the captured rebuild recipe.
- `SHA256SUMS`, `build-report.json` and `verification.json`.

Archive timestamps derive from the fixed source epoch. Member order and modes
are controlled. Volatile runner/tool evidence stays outside the deterministic
ZIPs. ZIP assembly uses a temporary directory, verifies both archives, then
publishes the new local output directory; failure creates no success marker.

CI extracts the verified portable ZIP with Windows-native tooling into a new
space-containing, non-ASCII path. With PATH limited to Windows and System32, it
checks QEMU/qemu-img versions, pinned `pc-q35-11.0`, WHPX presence and TCG absence.
The host uses the console executable with `CREATE_NO_WINDOW`; no windowless
variant is required or shipped. After successful build verification and Windows
inspection, the workflow uploads one engineering artifact named
`bridge-windows-runtime-<commit>-<attempt>` for seven days. It contains both ZIPs,
`SHA256SUMS`, `build-report.json` and `verification.json`; failed builds do not
upload partial output. No input caches or toolchain directories are uploaded.

Download that artifact from the specific successful run with
`gh run download <run-id> --name bridge-windows-runtime-<commit>-<attempt> --dir <new-directory>`.
Check the selected run and commit and both ZIP hashes against `SHA256SUMS` before
extracting the portable ZIP. Preserve the corresponding-source ZIP and notices.
These temporary CI artifacts are not release assets; retention does not change
`publicationReady: false` or establish native guest qualification.

## Portable app assembly

`runtime-inputs.json` pins the separate runtime/source archives, producer run,
seven-asset unpublished input draft and the app's runtime-manifest identity.
The input-only draft is never an app candidate and must never be published.
Acquisition verifies the exact draft target/inventory, successful producer run,
asset-ID readback and every reviewed hash. Uploaded `SHA256SUMS` is not authority.
A transfer failure stops visibly; there are no automatic retries or clobbers.

With existing operator GitHub CLI access, from the repository root:

```sh
python runtime/windows/delivery.py fetch --out NEW_INPUT_DIRECTORY
python runtime/windows/delivery.py check --inputs NEW_INPUT_DIRECTORY
python runtime/windows/delivery.py assemble --inputs NEW_INPUT_DIRECTORY --executable app/src-tauri/target/release/orka-bridge.exe --licenses app/public/licenses --release-tag APP_RELEASE_TAG --out NEW_DELIVERY_DIRECTORY
```

Assembly reruns the external-lock verifier and checks the compiled native pin.
`bridge-windows-x64.zip` contains the executable, app notices, `QEMU-SOURCE.txt`
and the complete unchanged runtime at `native/qemu`. Source is **not** inside
that app ZIP. The unchanged `bridge-windows-runtime-source.zip` is a separate
output: attach it to the same eventual app release. The source notice names its
hash and future published URL; draft walkers use that draft's asset list.
No source fetch happens at app startup. Keep all runtime notices intact.

The Image assets workflow has a separate, main-only explicit-dispatch path:
set `windows_candidate_tag` to the future app tag. The image profile is unused
on this path, and the image publisher jobs do not run. Only the trusted QEMU
input-fetch job has new `contents:write` permission (GitHub requires push access
to read drafts); it only reads inputs, verifies bytes and transfers a one-day
job artifact. The Windows build job has read-only permissions, rechecks all
inputs independently and uploads only the two delivery ZIPs. Its
`inspect-delivery.ps1` uses Windows-native extraction, verifies the full runtime
inventory and inspects QEMU with a Windows-only PATH. It does not boot a VM.
Ordinary PR CI uses fixtures, not a gigabyte input transfer. This packaging path
does not create a release/tag, activate app image pins or establish a native VM
walk; candidate publication and those walks are separate operations.

## Verify an output independently

```sh
python runtime/windows/verify.py --output OUTPUT --sources runtime/windows/sources.lock.json --packages runtime/windows/packages.lock.json
```

Use trusted external locks, not locks recovered from an untrusted archive. The
verifier never extracts or executes archive contents. It checks hashes, exact
inventories, AMD64 PE headers, recorded import closure, source/notice ownership,
firmware mapping and source Git blob/tree identities against those external
locks. Duplicate JSON keys, path aliases, unsafe Windows paths and unsupported
members fail. Runtime members are regular files only; any actual source links
must resolve inside the archive without dangling targets or cycles.

This is not binary-to-source attestation, a hermetic-build claim, legal approval
or future application download authentication. Self-described sums are not that
authentication. A trusted producer and stable local inputs remain assumptions.

Successful validation commands write one JSON value to stdout; failures write
no success JSON, explain the failure on stderr and exit 1. Usage errors exit 2.
`list-inputs` emits the selected file records as JSON; the internal `list-git`
helper emits four tab-separated columns for the build script. Neither prompts.

## Source representation and rebuilding

The source ZIP is the exact Windows build tree, not a POSIX checkout. Every Git
operation explicitly disables newline conversion and symlink materialization;
checkout attributes also prevent content filters. The inventory preserves
original modes, blob IDs, trees and gitlinks. Verification recomputes blob and
tree hashes, including the externally pinned post-patch QEMU tree.

The fork contains eleven mode-120000 C/Rust content blobs as well as three real
link targets. They remain ordinary blob-byte files. `provenance/git-index.json`
distinguishes multiline content placeholders from link placeholders. Never
blindly convert every mode-120000 entry into a link. To recreate the three real
links on POSIX, inspect their recorded target bytes and original modes first;
the other eleven entries are source content, not targets. The Windows build
must fail rather than silently repair a required link.

The source ZIP includes `recipe/build.sh`, both Python tools, this README, both
locks and the workflow. For a standard rebuild, use the captured workflow's
acquisition/install steps and invoke `bash recipe/build.sh INPUTS NEW_OUTPUT` in
the fresh pinned environment. Git acquisition is explicit; it does not depend on
retaining `.git` databases. The already patched `qemu/` tree and other source
roots also permit manual compilation using the configure/build flags in the
captured script. Official source archives remain opaque data; some contain
upstream VCS packs and are not installed as working Git configurations.

The selected patches and retained source checks come from the pinned
[Try Omarchy Windows recipe](https://github.com/omacom/try-omarchy-windows/tree/b830a873f29b907ebfbefb96b5cfc617cc01899a/runtime-build).
Its MIT copyright/permission notice is retained in both archives. QEMU and each
redistributed library/firmware family's separate notices and corresponding
sources remain beside the engineering bundle.
