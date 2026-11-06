# Contributing

Thanks for your interest in Offchart.

This repository is the public source of the deployed on-chain program. Its build is
meant to be **reproducible**, so changes are held to a strict bar:

- Anything under `programs/` changes the compiled binary and therefore the program hash.
  Behavioural changes to the program are not merged lightly.
- `rust-toolchain.toml` and `programs/offchart-lite/Cargo.lock` must stay as they are unless a
  change is explicitly intended.
- `programs/offchart-lite` must remain its own workspace (the `[workspace]` table in its
  `Cargo.toml`). Do not add a root `Cargo.toml`: moving the crate into a root workspace produces a
  different binary.

## Welcome contributions

- Documentation fixes and clarifications.
- Bug reports and questions via GitHub Issues.
- Review comments on the source.

For security issues, **do not** open an issue; follow [SECURITY.md](SECURITY.md).

By contributing you agree that your contributions are licensed under the [MIT License](LICENSE).
