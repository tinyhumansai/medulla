# Repository Guidelines

## What this repository is

The public face of Medulla: documentation and distribution. It holds no product
source code, and none should ever be added.

- `docs/` — engineering specs and protocol documents (`host-link-protocol.md`,
  `workflows.md`, `agent-harness-contract.md`, …), plans, and screenshots. Source
  comments in the Rust workspace refer to these by bare path (`docs/…`).
- `gitbooks/` — the GitBook sources published at
  [tinyhumans.gitbook.io/medulla](https://tinyhumans.gitbook.io/medulla). `SUMMARY.md`
  is the table of contents; a new page is not published until it is listed there.
- `install.sh` / `install.ps1` — the installers the README tells people to pipe
  into a shell. They resolve the newest GitHub Release here, verify its SHA-256
  against `latest.json`, and unpack it into `~/.medulla`.
- **Releases** — every published binary, its `.sha256`, and the `latest.json`
  manifest `medulla update` reads.

The Rust workspace that builds those binaries lives in
[`tinyhumansai/medulla-src`](https://github.com/tinyhumansai/medulla-src)
(private). Its `Release` workflow builds each target and publishes the packaged
artifacts here with a scoped GitHub App token. Only build output crosses that
boundary — never source.

## Working rules

Work on a branch and open a pull request; `main` is what the installers and the
GitBook integration read, so a broken `main` is immediately public.

- **Documentation lands here, code lands in `medulla-src`.** A change that needs
  both is two pull requests; land the code one first so the docs describe
  something that exists.
- **Link to source by URL, not by path.** The source tree is not in this
  repository, so `../../src/…` cannot resolve. Use
  `https://github.com/tinyhumansai/medulla-src/tree/main/src/…`. Links between
  documents *in this repository* stay relative.
- **Keep the boundary clean.** `scripts/check-public-boundary.sh` rejects private
  implementation names and internal provenance; it runs in CI on every pull
  request. Run it locally before pushing.
- **Do not edit installers casually.** `install.sh` and `install.ps1` are executed
  by strangers from a pipe. The `Install scripts` workflow exercises both against
  the real published release on every supported platform; wait for it.

## Checks

```sh
bash scripts/check-public-boundary.sh   # public-boundary guard
shellcheck --shell=sh install.sh        # what CI lints install.sh with
```

Never commit secrets or machine-local configuration.
