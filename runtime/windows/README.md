# Windows runtime source inputs

This is the input foundation for a headless, source-built Windows x64 QEMU
runtime. It does **not** build or download QEMU, apply patches, install MSYS2,
create archives, launch a VM or publish binaries. No working Windows backend
or release is claimed.

## Check the selected sources

Python 3.11 or newer is required, with no Python package dependencies. From the
repository root:

```sh
python -m unittest discover -s runtime/windows -p test_recipe.py -v
python runtime/windows/recipe.py check-sources --lock runtime/windows/sources.lock.json
```

Without `--inputs`, success means only that the lock has the expected shape,
immutable source URLs, patch order and required checks. JSON reports
`inputBytesVerified: false`, `filesVerified: 0` and `publicationReady: false`.
It does not prove that the commits exist, patches apply, dependencies build or
licenses permit distribution.

To check already acquired source files, add `--inputs /path/to/source-inputs`.
That directory must contain the lock's relative `patches/`, `checks/` and
`subprojects/` paths. Every listed regular file must match its size and SHA-256;
links and Windows reparse points are rejected. Keep the directory stable during
the check. Unrelated files are not examined: this is not an exact archive
inventory verifier or an app extraction boundary. The checker never downloads,
applies or executes source, including the retained test scripts.

Success exits 0 and prints one JSON record. Invalid inputs exit 1 with a reason
on stderr and no success record. Invalid CLI usage exits 2. A byte-verified
result describes these selected files only, not the QEMU Git tree or submodules.

## Selection and provenance

`sources.lock.json` uses `bridge-windows-sources/v1`:

- `reference` identifies Try Omarchy's immutable source recipe commit.
- `qemu` identifies the Windows QEMU fork commit and its upstream 11.0.0 base.
- `sourceDateEpoch` preserves the reference's fixed packaging epoch for later use.
- `patches` gives six ordered patches with exact source URLs, sizes and digests.
- `checks` pins the standalone socket-close, fd-setsize and WHPX CPU-model tests.
- `submodules` records the QEMU commit's top-level Git links and repository URLs.
- `wraps` inventories the selected commit's Meson wrap-file bytes. It does not
  approve automatically downloading the dependencies described inside them.

Patch purposes: 0001 tolerates nesting refusal; 0005 restores Windows socket
handle flags; 0009 uses UTF-8 paths; 0015 reclaims WHPX pages; 0019 honors CPU
models; 0024 increases slirp socket capacity. The future headless profile keeps
0015 dormant without a balloon device. Guest KVM is not a requirement. The
patches and test scripts have not been applied or run by this foundation.

Expected patch digests come from the reference's
[committed source lock](https://github.com/omacom/try-omarchy-windows/blob/b830a873f29b907ebfbefb96b5cfc617cc01899a/runtime-build/sources.lock.json).
Check-script sizes/digests were computed from the exact immutable source URLs
in this lock. Git-link commits come from the QEMU commit's tree and `.gitmodules`;
wrap digests were computed from that same commit's files. Review these selections
as source changes. A digest is integrity evidence, not an upstream signature or
permission to run a downloaded file.

The reference recipe is [Try Omarchy](https://github.com/omacom/try-omarchy-windows/tree/b830a873f29b907ebfbefb96b5cfc617cc01899a/runtime-build),
licensed MIT. QEMU remains GPL-2.0 and each dependency and firmware component
retains its own terms. This directory redistributes source-selection metadata,
not their runtime binaries. Later packaging must provide all applicable notices,
corresponding sources and build material.

## Follow-up boundaries

The complete MSYS2 package closure is deliberately not represented by a partial
or placeholder lock here. A later input slice must pin the dated bootstrap,
its preinstalled inventory, additional package archives and final installed
inventory, including required source provenance. Subsequent work adds archive
verification, the headless recipe and actual read-only Windows compilation.
Only that build can prove clean patch application, effective features and DLL
closure; only a separate native smoke run can prove VM operation.

CI checks this foundation on Linux and Windows using local synthetic fixtures
and the checked-in source lock. It downloads no runtime inputs and uploads no
binaries. A host without symlink privileges may skip that one filesystem test;
Linux CI exercises it. No Windows ACL, native-launch, WHPX or guest claim follows.
