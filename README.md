<div align="center">

# Offchart

**Direct P2P token swaps on Solana that never touch the pool.**

[![Solana](https://img.shields.io/badge/Solana-mainnet-9945FF?logo=solana&logoColor=white)](https://solscan.io/account/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P)
[![Rust](https://img.shields.io/badge/Rust-1.89-orange?logo=rust&logoColor=white)](rust-toolchain.toml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Program ID](https://img.shields.io/badge/program-8vbugQ1P…bo2P-14F195)](https://explorer.solana.com/address/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P?cluster=mainnet-beta)

[Website](https://useoffchart.xyz) · [Solscan](https://solscan.io/account/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P) · [Security](SECURITY.md)

</div>

Docs: coming soon <!-- DOCS_URL -->

---

## Overview

Offchart is a Solana program that settles token swaps **directly between two or three users**, at a
price checked against an oracle, without routing through a liquidity pool. Funds stay in the user's own
token account until the moment of settlement.

The program is written in plain Rust on [`pinocchio`](https://crates.io/crates/pinocchio) 0.12.0 — no
Anchor, no IDL, no events. It keeps the byte layout of the earlier Anchor design: 8-byte discriminators,
the `Config` / `Order` / `TokenConfig` account layouts, the PDA seeds, and error codes `6000 + N`.

## Program

| | |
|---|---|
| **Network** | Solana mainnet-beta |
| **Program ID** | `8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P` |
| **Explorers** | [Solscan](https://solscan.io/account/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P) · [Solana Explorer](https://explorer.solana.com/address/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P?cluster=mainnet-beta) |
| **Deploy transaction** | [Solscan](https://solscan.io/tx/NxcjUbm4kztdS1fNRH1owqTwvbCw8W8bQ7WtJaFdAbyYPt99PUmx84kstxjxjNqn4ndAzj3cmw3FGMGFanK2jLd) · [Solana Explorer](https://explorer.solana.com/tx/NxcjUbm4kztdS1fNRH1owqTwvbCw8W8bQ7WtJaFdAbyYPt99PUmx84kstxjxjNqn4ndAzj3cmw3FGMGFanK2jLd) |
| **ProgramData** | [`83WaUsj7jRA3eXxqHhVBeVwz1i16VnmbN2Fx9EPrerQN`](https://solscan.io/account/83WaUsj7jRA3eXxqHhVBeVwz1i16VnmbN2Fx9EPrerQN) |
| **Upgrade authority** | [`4WfpzfRrhC3idmkWjfJpHCyVbxcSjesDpPncz3hZgHer`](https://solscan.io/account/4WfpzfRrhC3idmkWjfJpHCyVbxcSjesDpPncz3hZgHer) |
| **Website** | <https://useoffchart.xyz> |

## How it works

1. **Allowlist.** An admin allowlists (or blocks) token mints. Each mint gets a `["token", mint]` PDA
   holding its status, pool vaults and price tolerance.
2. **Create an order.** A user states what they give (mint and total) and what they want (one or more
   mints), with a limit (`limit_bps` or an absolute limit), a minimum fill, a partial-fill flag and an
   expiry. Tokens **do not move**: they stay in the user's token account, approved to the `delegate` PDA.
   The order is non-custodial until settlement.
3. **Settle.** A keeper matches 2–3 participants and calls `settle` with the fill amounts. The
   transaction must carry an **ed25519-signed oracle `PriceBatch`** (domain `offchart.pricebatch.v1`) in
   the previous instruction, no older than `max_age` slots.
4. **Checks and transfer.** The program verifies each user's price limit, the deviation from the pool
   price, the token program (SPL Token or Token-2022; unsupported Token-2022 extensions are rejected) and
   the recipients. Funds then move with `transfer_checked`, signed by the `delegate` PDA, and the fee is
   sent to the treasury.
5. **Cancel anytime.** The owner can cancel an order; the keeper can cancel expired or invalid ones.

## Instructions

| Instruction | Caller | Description |
|---|---|---|
| `init_config(keeper, oracle, treasury, max_age)` | Upgrade authority | One-time setup. Only the program's upgrade authority can call it (checked through ProgramData), so it cannot be front-run. Creates the `config`, `delegate` and `escrow` PDAs. `max_age ≤ 300`. Fee starts at 0. |
| `set_config` | Admin | Updates keeper, oracle, treasury, fee, pause flag and `max_age`. Fee is capped on-chain at **300 bps**. |
| `upsert_token_config` | Admin | Allowlists a mint, or updates its `TokenConfig` (status, pool vaults, tolerance). |
| `block_token` | Admin | Blocks a mint. |
| `create_order` | User | Creates an order (see above). Funds stay in the user's account, delegated to the `delegate` PDA. |
| `cancel_order` | Order owner | Cancels the owner's order. |
| `keeper_cancel_order` | Keeper | Cancels expired or invalid orders. |
| `settle(fills: Vec<u64>)` | Keeper | Settles 2–3 participants against a fresh signed oracle price batch. |

**Fees.** The fee is `floor(amount * bps / 10000)`, computed without 128-bit division (`math.rs`, covered
by host tests). It is a configuration value set through `set_config`, not a constant in the code. The
intended operational fee is 50 bps (0.5%) from each side; the mainnet configuration step has not been
performed yet, so on-chain the fee is currently 0 or not initialized.

## Accounts and PDAs

| PDA | Seeds | Purpose |
|---|---|---|
| `config` | `["config"]` | Global settings: keeper, oracle, treasury, fee, pause, `max_age`. |
| `delegate` | `["delegate"]` | Authority approved on users' token accounts; signs `transfer_checked` at settlement. |
| `escrow` | `["escrow"]` | Program account created at `init_config`. |
| `token config` | `["token", mint]` | Per-mint status, pool vaults and tolerance. |
| `order` | `["order", owner, nonce]` | A user's order. |

**`settle` account order:** `keeper`, `config`, `delegate`, `escrow`, instructions sysvar, system program,
then **10 accounts per participant**.

## Reproducible build and verification

The build is pinned so anyone can reproduce the deployed binary.

- Agave / `cargo-build-sbf` 3.1.10, platform-tools v1.52
- `rustc` 1.89.0 (pinned by [`rust-toolchain.toml`](rust-toolchain.toml))

### Build locally

```bash
cd programs/offchart-lite
cargo build-sbf --manifest-path Cargo.toml -- --locked
sha256sum target/deploy/offchart_lite.so
# b01199aff6b7cc8779920abc611353961a25e0d67b0ae98caa6d3e347bb9240b  (41760 bytes)
```

### Compare with mainnet

```bash
# hash of your local build
solana-verify get-executable-hash target/deploy/offchart_lite.so
# hash of the deployed program
solana-verify -u https://api.mainnet-beta.solana.com get-program-hash 8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P
# both: 009b7b28f7fab5afeb113cba546f988f3c1a1af20e77b0e949137429aee01c72
```

### Docker verifiable build

> The Docker flow below has **not been run yet**; the local build above is what has been verified.

```bash
solana-verify build --library-name offchart_lite \
  --base-image solanafoundation/solana-verifiable-build@sha256:f71be5ca7620b7e40933b7f1294fa44e01d08c1fc5ba1f375a2478f5a01580d3

solana-verify verify-from-repo -u https://api.mainnet-beta.solana.com \
  --program-id 8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P \
  https://github.com/sipLinchik/offchart-program \
  --commit-hash <COMMIT> --library-name offchart_lite --mount-path programs/offchart-lite
```

> **Keep the crate its own workspace.** `programs/offchart-lite/Cargo.toml` contains its own
> `[workspace]` table. Moving the crate into a root workspace produces a different binary, so there is
> intentionally no root `Cargo.toml`, and the mount path is `programs/offchart-lite`.

## Repository layout

```text
.
├── programs/
│   └── offchart-lite/
│       ├── Cargo.toml        # standalone workspace
│       ├── Cargo.lock
│       └── src/
│           ├── lib.rs
│           ├── program.rs    # entrypoint, settle
│           ├── ops.rs        # config, token config, order instructions
│           └── math.rs       # fee math (host-tested)
├── rust-toolchain.toml       # Rust 1.89.0
├── LICENSE
├── SECURITY.md
└── CONTRIBUTING.md
```

## License

[MIT](LICENSE) © 2026 Offchart
