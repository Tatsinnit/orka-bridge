# Bridge

Bridge is a Tauri 2 desktop app with a React/TypeScript frontend under `app/`
and a Rust shell under `app/src-tauri/`.

## Scope

Keep changes focused. The current shell has Overview, Designer and Clusters,
with empty screens and no engine connection. Do not imply setup, testing or
deployment works until it is implemented and verified.

Preserve the Microsoft project files and all third-party notices. Do not edit
generated lockfiles by hand. Keep application code, tests and docs portable.

## Checks

From `app/`, use the pinned Node/npm versions in CONTRIBUTING.md:

```sh
npm ci
npm run lint
npm test
npm run build
cargo +1.98.1 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo +1.98.1 test --release --manifest-path src-tauri/Cargo.toml --locked --features custom-protocol
cargo +1.98.1 clippy --release --manifest-path src-tauri/Cargo.toml --locked --all-targets --features custom-protocol -- -D warnings
```

Build frontend assets before Rust checks: Tauri embeds them.
Run `actionlint .github/workflows/checks.yml` after workflow changes.
CI builds Windows x64 and Apple silicon Mac. Walk visible changes in a native
build; a browser preview alone is not native verification.
