# Offchart

Direct P2P token swaps on Solana that do not touch the pool.

[![Solana](https://img.shields.io/badge/Solana-mainnet-9945FF?logo=solana&logoColor=white)](https://explorer.solana.com/address/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P?cluster=mainnet-beta)
[![Rust](https://img.shields.io/badge/Rust-1.89.0-000000?logo=rust&logoColor=white)](rust-toolchain.toml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Program](https://img.shields.io/badge/program-8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P-111111)](https://solscan.io/account/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P)

Website: [https://useoffchart.xyz](https://useoffchart.xyz)

**Docs:** <!-- DOCS_URL --> Documentation URL will be added here.

This repository is the public source of the Offchart on-chain program, published so the deployed mainnet binary can be rebuilt and checked with [solana-verify](https://github.com/Ellipsis-Labs/solana-verifiable-build).

## Program

| | |
| --- | --- |
| Program | [`8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P`](https://solscan.io/account/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P) · [Solscan](https://solscan.io/account/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P) · [Solana Explorer](https://explorer.solana.com/address/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P?cluster=mainnet-beta) |
| Deploy transaction | [`NxcjUbm4kztdS1fNRH1owqTwvbCw8W8bQ7WtJaFdAbyYPt99PUmx84kstxjxjNqn4ndAzj3cmw3FGMGFanK2jLd`](https://solscan.io/tx/NxcjUbm4kztdS1fNRH1owqTwvbCw8W8bQ7WtJaFdAbyYPt99PUmx84kstxjxjNqn4ndAzj3cmw3FGMGFanK2jLd) · [Solscan](https://solscan.io/tx/NxcjUbm4kztdS1fNRH1owqTwvbCw8W8bQ7WtJaFdAbyYPt99PUmx84kstxjxjNqn4ndAzj3cmw3FGMGFanK2jLd) · [Solana Explorer](https://explorer.solana.com/tx/NxcjUbm4kztdS1fNRH1owqTwvbCw8W8bQ7WtJaFdAbyYPt99PUmx84kstxjxjNqn4ndAzj3cmw3FGMGFanK2jLd) |
| Upgrade authority | [`4WfpzfRrhC3idmkWjfJpHCyVbxcSjesDpPncz3hZgHer`](https://solscan.io/account/4WfpzfRrhC3idmkWjfJpHCyVbxcSjesDpPncz3hZgHer) · [Solscan](https://solscan.io/account/4WfpzfRrhC3idmkWjfJpHCyVbxcSjesDpPncz3hZgHer) · [Solana Explorer](https://explorer.solana.com/address/4WfpzfRrhC3idmkWjfJpHCyVbxcSjesDpPncz3hZgHer?cluster=mainnet-beta) |
| Program data | [`83WaUsj7jRA3eXxqHhVBeVwz1i16VnmbN2Fx9EPrerQN`](https://solscan.io/account/83WaUsj7jRA3eXxqHhVBeVwz1i16VnmbN2Fx9EPrerQN) · [Solscan](https://solscan.io/account/83WaUsj7jRA3eXxqHhVBeVwz1i16VnmbN2Fx9EPrerQN) · [Solana Explorer](https://explorer.solana.com/address/83WaUsj7jRA3eXxqHhVBeVwz1i16VnmbN2Fx9EPrerQN?cluster=mainnet-beta) |

Cluster: Solana mainnet-beta (`https://api.mainnet-beta.solana.com`).

## Overview

Offchart is a Solana program for direct peer-to-peer token swaps that skip the pool. Settlement moves tokens between the participants. When a token config records pool vaults, the attested price must stay within that token's tolerance of the pool price. The pool is a price check, and the swap itself does not trade against it.

The program is plain Rust on pinocchio 0.12.0, with no Anchor, no IDL, and no events. It keeps the byte layout of the earlier Anchor design: 8-byte discriminators, the same `Config`, `Order`, and `TokenConfig` layouts, the same PDA seeds, and custom errors numbered `6000 + n`.

Open orders are non-custodial. Tokens stay in the user's token account, approved to the `delegate` PDA, until settlement.

The fee is a config value. `init_config` writes `0`. `set_config` is what sets it, and the program rejects a fee above 300 bps. The operating fee is 50 bps (0.5%) from each side. That number is stored by `set_config`. It is not a constant in the source. Mainnet bootstrap is not done yet, so on-chain the fee is still 0, or config is not initialized.

## Instructions

`settle` is implemented in `programs/offchart-lite/src/program.rs`. The other instructions are in `src/ops.rs`.

| Instruction | Caller | Effect |
| --- | --- | --- |
| `init_config(keeper, oracle, treasury, max_age)` | Upgrade authority | Creates the `config` account and binds the `delegate` and `escrow` PDAs. `max_age` is 1–300 slots. The caller is checked against ProgramData, so the instruction cannot be front-run. Fee starts at 0. Not paused. |
| `set_config` | Admin | Updates any of admin, keeper, oracle, treasury, fee, pause, and max age. Fee must be ≤ 300 bps. Max age, when set, must be 1–300 slots. |
| `upsert_token_config` | Admin | Allowlists a mint or updates its config. PDA `["token", mint]` holds status, the token program, pool vaults, and tolerance. Status `0` is the allowed state. |
| `block_token` | Admin | Sets that mint's token-config status to `1` (blocked). |
| `create_order` | User | Opens an order: give mint, total, 1–5 wanted mints, `limit_bps`, absolute limit, minimum fill, partial flag, and expiry. The source token account stays with the user and must already delegate at least `total` to the `delegate` PDA. |
| `cancel_order` | Order owner | Closes the order, returns its rent to the owner, and can revoke the token-account delegate. |
| `keeper_cancel_order` | Keeper or admin | Closes an order and returns its rent to the owner. Used for orders that should not stay open, such as expired or invalid ones. This path does not itself check expiry. |
| `settle(fills)` | Keeper | Settles 2 or 3 orders. See below. |

`create_order` accepts three kinds:

- `0` — give mint is not native SOL, exactly one wanted mint, and that mint is native SOL.
- `1` — give mint is native SOL, no wanted mint is native SOL, and the absolute limit is greater than 0. The price check uses the buyer rule.
- `2` — no further kind-specific rule. The price check uses the seller rule, as it does for kind `0`.

### Settle

`settle` takes a Borsh `Vec<u64>` of fill amounts and requires 2 or 3 participants. The keeper must sign, and the program must not be paused.

The instruction immediately before `settle` must be an ed25519 verify of an oracle `PriceBatch`. The signed message uses the domain `offchart.pricebatch.v1`, is bound to this program id, carries at most 16 prices, and its slot must be within `max_age` of the current slot. Prices are lamports × 1e12 per raw unit. Native SOL is priced at 1e12.

For each participant the program checks the order is still active and unexpired, the fill size (partial fills and the minimum fill), the user's price limit, deviation from pool price when vaults are set, the token program (SPL Token or Token-2022; a mint with extensions is rejected), and the recipient. It then moves funds with `transfer_checked` signed by the `delegate` PDA and sends the fee to the treasury token account. Native SOL is unwrapped to the recipient wallet. Any failure reverts the whole transaction.

Account order:

1. keeper (signer)
2. config
3. delegate
4. escrow
5. instructions sysvar
6. system program
7. ten accounts per participant: order, source token account, destination, mint, token program, token config, temporary wSOL account, treasury token account, pool vault (base), pool vault (quote)

Fee math, in `src/math.rs`, is `floor(amount * bps / 10_000)`, with no 128-bit division.

Custom program errors are `6000 + n`, using the indexes in `src/program.rs` and `src/ops.rs` (`NOT_ADMIN` = 6000 through `NOT_OWNER` = 6023).

## Accounts

| Account | Seeds | Role |
| --- | --- | --- |
| Config | `["config"]` | 150-byte account. Admin, keeper, oracle, treasury, fee (u16 bps), paused flag, max age (slots), and the config / delegate / escrow bumps. Created by `init_config`. |
| Delegate | `["delegate"]` | PDA signer for `transfer_checked`. Bump is stored on config. Users approve this address on their token accounts. |
| Escrow | `["escrow"]` | PDA checked by `init_config` and passed on `settle`. |
| Token config | `["token", mint]` | 140-byte account. Mint, status, token program, pool vaults, tolerance (bps), bump. |
| Order | `["order", owner, nonce_le]` | 289-byte account. Owner, kind, give mint, total, filled, up to five wanted mints, limits, minimum fill, partial flag, expiry, status, nonce, bump. |
| Temporary wSOL | `["wsol", order]` | Created during `settle` when the paid mint is native SOL, then closed. Rent is funded by the keeper. |

Order status `2` means fully filled and cannot settle again. A partial fill is status `1`. `cancel_order` and `keeper_cancel_order` close the account.

## Reproducible build

The mainnet program was built with Agave `cargo-build-sbf` 3.1.10, platform-tools v1.52, and rustc 1.89.0. `rust-toolchain.toml` pins rustc 1.89.0. `programs/offchart-lite` is its own Cargo workspace (`[workspace]` in that crate's `Cargo.toml`). Keep it that way. A root workspace produces a different binary, so this repository has no root `Cargo.toml`.

```bash
cd programs/offchart-lite
cargo build-sbf --manifest-path Cargo.toml -- --locked
sha256sum target/deploy/offchart_lite.so
solana-verify get-executable-hash target/deploy/offchart_lite.so
solana-verify -u https://api.mainnet-beta.solana.com \
  get-program-hash 8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P
```

| Check | Expected |
| --- | --- |
| `offchart_lite.so` sha256 | `b01199aff6b7cc8779920abc611353961a25e0d67b0ae98caa6d3e347bb9240b` |
| File size | 41,760 bytes |
| `solana-verify get-executable-hash` | `009b7b28f7fab5afeb113cba546f988f3c1a1af20e77b0e949137429aee01c72` |
| `get-program-hash` on mainnet | the same `009b7b28f7fab5afeb113cba546f988f3c1a1af20e77b0e949137429aee01c72` |

Pass `--locked` so the build uses `programs/offchart-lite/Cargo.lock`.

## Verification

The locked build above matches the program deployed on mainnet: the on-chain ELF is 41,760 bytes and its sha256 is the value in the table. The Verified badge on Solscan and Solana Explorer comes from a `solana-verify` Docker build and a remote job. That job has not been submitted, so this repository does not claim the badge.

`<COMMIT>` is the 40-character git SHA of the commit that contains this source, which is the `main` commit once it is published. `solana-verify` checks out that commit and builds `programs/offchart-lite`.

```bash
solana-verify build \
  --library-name offchart_lite \
  --base-image solanafoundation/solana-verifiable-build@sha256:f71be5ca7620b7e40933b7f1294fa44e01d08c1fc5ba1f375a2478f5a01580d3

solana-verify verify-from-repo \
  -u https://api.mainnet-beta.solana.com \
  --program-id 8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P \
  https://github.com/siplinchik/offchart-program \
  --commit-hash <COMMIT> \
  --library-name offchart_lite \
  --mount-path programs/offchart-lite

solana-verify remote submit-job \
  --program-id 8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P \
  --uploader 4WfpzfRrhC3idmkWjfJpHCyVbxcSjesDpPncz3hZgHer
```

`--mount-path` stays `programs/offchart-lite`.

As of solana-verify 0.5.2, `--remote` is deprecated. `remote submit-job` writes the verification record and needs a mainnet transaction signed by the upgrade authority (`--uploader`).

## Repository layout

```
.gitignore
rust-toolchain.toml                  # rustc 1.89.0
programs/offchart-lite/              # standalone Cargo workspace — do not add a root workspace
  Cargo.toml
  Cargo.lock
  src/lib.rs                         # crate root
  src/math.rs                        # fee, price, and fill checks
  src/ops.rs                         # admin and order-lifecycle instructions
  src/program.rs                     # entrypoint and settle
CONTRIBUTING.md
LICENSE                              # MIT, copyright 2026 Offchart
README.md
SECURITY.md
```

## Disclaimer

This program has not been audited. A byte-identical build is not an audit. The Verified badge, once obtained, means the deployed binary matches this source. It is not a security review.

The program is upgradeable. The upgrade authority is `4WfpzfRrhC3idmkWjfJpHCyVbxcSjesDpPncz3hZgHer`.

The fee is set via `set_config` and capped at 300 bps. It is not hard-coded. Until that config is written on mainnet, the on-chain fee is 0 or config is not initialized. The operating value is 50 bps from each side, as a config setting.

## License

[MIT](LICENSE). Copyright 2026 Offchart.
