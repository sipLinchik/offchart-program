# Contributing

This repository is the public, verifiable source of the Offchart on-chain program.

`programs/offchart-lite` is its own Cargo workspace. Do not add a root `Cargo.toml`, and do not move the crate into another workspace. Doing so changes the compiled `.so`, so it will no longer match mainnet.

Leave the program tree as published when the goal is to reproduce the deployed binary:

- `.gitignore`
- `rust-toolchain.toml`
- `programs/offchart-lite/**`

`Cargo.toml` is part of that tree and has no `license` field. The MIT license for the repository is [`LICENSE`](LICENSE). Editing `Cargo.toml` to add metadata changes the verified source.

Documentation fixes — `README.md`, `SECURITY.md`, and this file — are welcome as pull requests.

Report vulnerabilities through [GitHub Security Advisories](https://github.com/siplinchik/offchart-program/security/advisories/new). See [SECURITY.md](SECURITY.md).
