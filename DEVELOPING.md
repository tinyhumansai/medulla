# Developing Medulla

The Rust workspace, documentation, installers, and GitHub Actions live in this
repository. The hosted orchestration service is operated separately; the
`--mock` runtime supports offline development and tests.

## Quick start

```sh
make init                       # initialize vendored crates and Rust tooling
cargo run                       # start the TUI
cargo run -- --mock             # run the offline demo
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --all-targets
```

See [Contributing](gitbooks/developers/contributing.md) for the detailed test,
coverage, and contribution workflow. The engineering specifications referenced
by source comments are in [`docs/`](docs/).

## Releases

The **Release** workflow is dispatched from `main`. It reads the workspace
version, creates a matching `vX.Y.Z` tag, builds packages for Linux x86_64,
Linux aarch64, macOS Apple Silicon, and Windows x86_64, then publishes those
artifacts and `latest.json` as a GitHub Release in this repository. The workflow
uses the repository's standard `GITHUB_TOKEN`; it requires no organization app
secrets. Bump the workspace version and lockfile in a reviewed pull request
before dispatching a release.
