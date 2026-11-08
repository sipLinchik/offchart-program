<div align="center">

# Offchart

**Direct P2P token swaps on Solana that never touch the pool.**

[![Solana](https://img.shields.io/badge/Solana-mainnet-9945FF?logo=solana&logoColor=white)](https://solscan.io/account/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P)
[![Rust](https://img.shields.io/badge/Rust-1.89-orange?logo=rust&logoColor=white)](rust-toolchain.toml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Program ID](https://img.shields.io/badge/program-8vbugQ1P…bo2P-14F195)](https://explorer.solana.com/address/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P?cluster=mainnet-beta)

[Website](https://useoffchart.xyz) · [Docs](https://offchart.gitbook.io/offchart/) · [Solscan](https://solscan.io/account/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P) · [Security](SECURITY.md)

</div>

---

## Overview

Offchart is a Solana program that settles token swaps **directly between two or three users**, without routing
through a liquidity pool. Funds stay in the user's own token account until the moment of settlement.

Any compatible SPL Token or Token-2022 mint can be traded **without registration**: there is no allowlist, no
per-mint account, no pool or liquidity requirement. Each order carries the owner's own signed minimum
(a *floor*, in raw token units, net of fees), and the program refuses any settlement that would give a user less.

The program is written in plain Rust on [`pinocchio`](https://crates.io/crates/pinocchio) 0.12.0 — no Anchor, no IDL,
no events.

## Program

| | |
|---|---|
| **Network** | Solana mainnet-beta |
| **Program ID** | `8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P` |
| **Explorers** | [Solscan](https://solscan.io/account/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P) · [Solana Explorer](https://explorer.solana.com/address/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P?cluster=mainnet-beta) |
| **Initial deploy transaction** | [Solscan](https://solscan.io/tx/NxcjUbm4kztdS1fNRH1owqTwvbCw8W8bQ7WtJaFdAbyYPt99PUmx84kstxjxjNqn4ndAzj3cmw3FGMGFanK2jLd) · [Solana Explorer](https://explorer.solana.com/tx/NxcjUbm4kztdS1fNRH1owqTwvbCw8W8bQ7WtJaFdAbyYPt99PUmx84kstxjxjNqn4ndAzj3cmw3FGMGFanK2jLd) |
| **ProgramData** | [`83WaUsj7jRA3eXxqHhVBeVwz1i16VnmbN2Fx9EPrerQN`](https://solscan.io/account/83WaUsj7jRA3eXxqHhVBeVwz1i16VnmbN2Fx9EPrerQN) |
| **Upgrade authority** | [`4WfpzfRrhC3idmkWjfJpHCyVbxcSjesDpPncz3hZgHer`](https://solscan.io/account/4WfpzfRrhC3idmkWjfJpHCyVbxcSjesDpPncz3hZgHer) |
| **Website** | <https://useoffchart.xyz> |

The deploy transaction above is the **initial** deployment. Later upgrades are separate transactions signed by the upgrade
authority; the source in this repository describes the current code, and the state of the live program is what
[Compare with mainnet](#compare-with-mainnet) lets you check for yourself.

## How it works

1. **Create an order.** A user states what they give (mint and total) and what they want (up to five mints),
   with, for each wanted mint, the minimum amount they accept for the **whole** order (`min_out`, raw units, after fees),
   a minimum fill, a partial-fill flag and an expiry. Tokens **do not move**: they stay in the user's token account,
   approved to the `delegate` PDA. The order is non-custodial until settlement.
2. **Settle.** A keeper matches 2–3 participants and calls `settle` with the fill amounts. For every participant the
   program checks the order, the mint (and, for Token-2022, its extensions), the delegation and balance, the
   recipient account and that the amount received **after the fee** still satisfies the owner's floor
   (`net · total ≥ min_out · fill`, exact integer arithmetic). Funds then move with `transfer_checked`, signed by the
   `delegate` PDA, and the fee goes to the treasury. Everything is atomic: any failed check reverts the whole transaction.
3. **Cancel anytime.** The owner can cancel an order; the keeper can cancel expired or invalid ones.

The price between two tokens is the one the two users agree on through their floors — the program does not compute a
"fair" price of an arbitrary token and there is no pool, liquidity or market-cap gate.

### Order value cap

Every order has a cap on the **cumulative USD value actually settled against it** (the default is **$100**, stored in
`Config` and changeable only by the admin key). The program does not guess what an arbitrary token is worth; it measures
the value on an *anchor* leg — one of **wSOL, USDC or USDT** — using an independent price:

* for each participant the value of a fill is the larger of the gross anchor amount it gives and the gross anchor amount it
  receives, valued at **price + confidence**, rounded up, in 1e-6 USD;
* the running total is stored in the order (bytes 252..260) and `used + value ≤ cap` is required for every fill, across all
  partial fills and all counterparties;
* a participant with **no anchor on either of its two legs** (unknown for unknown) cannot trade;
* the cap is per order, not per wallet, and it is not a statement about the intrinsic market value of an unlisted token.

Compatible tokens still trade permissionlessly: they trade **against** an anchor (for example `TOKEN ↔ USDC`, `TOKEN ↔ SOL`,
or a three-party ring that contains an anchor next to every participant).

### Price source and trust

The anchor prices are read on-chain from the **canonical sponsored push-feed accounts** of Pyth Network (shard 0) in the same
transaction — the program has no oracle key and the keeper does not sign any price.

| Anchor | Canonical price account | Pyth feed id |
|---|---|---|
| SOL/USD | `7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE` | `ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d` |
| USDC/USD | `Dpw1EAVrSB1ibxiDQyTAW6Zip3J4Btk2x4SgApQCeFbX` | `eaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a` |
| USDT/USD | `HT2PLQBcG5EiCcNSaMHAjSgd9F98ecpATbk4Sk5oYuM` | `2b89b9dc8fdf9f34709a5b106b472f0f39bb6ca9ce04b0fd7f2e971688e2e53b` |

The accounts are the program-derived addresses of the Pyth push oracle (`pythWSnswVUd12oZpeFP8e9CVaEqJg25g1Vtc2biRsT`, seeds
`[0u16 LE, feed id]`). The program accepts an account **only at its pinned address** and then requires: owner = the Pyth
receiver (`rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`), not executable, the exact 134-byte `PriceUpdateV2` shape and
discriminator, verification level **Full**, the matching feed id, `write_authority` equal to the account itself,
`posted_slot ≤ current slot`, positive price, exponent in `[-12, 0]`, a publish time that is neither in the future (beyond a 10 s
tolerance) nor older than the configured freshness (seconds, default 90, at most 300), and a confidence interval of at most 1 %
of the price. The anchor mints themselves must be the canonical SPL mints with their known decimals (wSOL 9, USDC 6, USDT 6).

What this does and does not mean:

* The price is published and maintained by Pyth, independent of the keeper and of any Offchart signer. Under Pyth's current
  documentation the core contracts verify a signed price inside each transaction through Pyth Pro routers; this repository
  does not re-verify that machinery — it relies on the account at the canonical address, owned by the receiver, carrying
  verification level Full.
* Reading the accounts on-chain needs no key. The push feeds are sponsored by the Pyth Data Association; their documentation
  gives a heartbeat of about one minute and a 0.5 % deviation threshold and states that updates may be delayed. **Availability
  is not guaranteed**: if a feed is stale or missing, settlements that need it fail (they are never priced from anything else).
* Because the program reads the single canonical current account, a caller cannot substitute an older or different update, and
  a per-order price nonce is unnecessary. This describes the on-chain read; it is not a promise of an economically
  "correct" price — the confidence interval is Pyth's statement, not a guarantee.

## Instructions

| Instruction | Caller | Description |
|---|---|---|
| `init_config(keeper, limits, treasury, max_age)` | Upgrade authority | One-time setup. Only the program's upgrade authority can call it (checked through ProgramData), so it cannot be front-run. Creates the `config` account and verifies the derived `delegate` and `escrow` addresses. `limits` is the order-cap record (below); `max_age` is the price freshness in **seconds**, `1..=300`. Fee starts at 0. |
| `set_config` | Admin | Updates keeper, limits, treasury, fee, pause flag and `max_age`. Fee is capped on-chain at **300 bps**. |
| `create_order` | User | Creates an order (see above). Refused if `Config` does not carry a valid limits record. Funds stay in the user's account, delegated to the `delegate` PDA. |
| `cancel_order` | Order owner | Cancels the owner's order. |
| `keeper_cancel_order` | Keeper | Cancels expired or invalid orders. |
| `settle(fills: Vec<u64>)` | Keeper | Settles 2–3 participants. |

There is no token registry and no price-batch instruction: mints are checked directly from their on-chain data.

**Fees.** The fee is `floor(amount * bps / 10000)`, computed without 128-bit division. It is a configuration value set through
`set_config`, not a constant in the code. The intended operational fee is 50 bps (0.5 %) from each side; the maximum the program
allows is 300 bps. The fee is always sent to the treasury token account: if that account cannot receive (for example it is
frozen), the settlement reverts — there is no way around the fee. This README does not describe the current on-chain fee value;
read the `Config` account for that.

## Accounts and PDAs

| Account | Seeds / layout | Purpose |
|---|---|---|
| `config` (150 B) | `["config"]` | keeper, limits record, treasury, fee, pause flag, price freshness in seconds, bumps. |
| `delegate` | `["delegate"]` | Authority approved on users' token accounts; signs `transfer_checked` at settlement. Derived only. |
| `escrow` | `["escrow"]` | Derived address (reserved). |
| `order` (329 B) | `["order", owner, nonce]` | A user's order: owner, give mint, total and filled amount, up to five wanted mints with their `min_out`, minimum fill, partial flag, expiry, status and the cumulative USD value used. |
| temporary wSOL | `["wsol", order]` | Short-lived token account used by `settle` to pay native SOL. |

**Limits record** (the former oracle-key slot of `Config`, 32 bytes): `cap` as `u64` little-endian in 1e-6 USD (default
`100_000_000` = $100) · the ASCII marker `OFFCAPv1` · 16 zero bytes. The record is validated on every `settle`, `create_order`,
`init_config` and `set_config`; a `Config` that still holds a plain key in that slot is refused (error `6043`), never read as a cap.

**`settle` accounts:** `keeper`, `config`, `delegate`, `system program`, then **7 accounts per participant** (order, source token
account, destination, mint, token program, temporary wSOL account, treasury token account), then **0–3** of the canonical Pyth
price accounts above — exactly the anchors that appear on the give legs.

**Errors:** `6000 + N`; rCap-era additions: `6040` cap exceeded, `6041` unpriced pair (unknown for unknown), `6042` invalid price
account or anchor mint, `6043` `Config` not in the current format.

## Reproducible build and verification

The build is pinned so anyone can reproduce the binary from this source.

- Agave / `cargo-build-sbf` 3.1.10, platform-tools v1.52, SBF v0
- `rustc` 1.89.0 (pinned by [`rust-toolchain.toml`](rust-toolchain.toml)), dependencies locked (`--locked`)
- Verifiable-build image, pinned by digest:
  `solanafoundation/solana-verifiable-build@sha256:f71be5ca7620b7e40933b7f1294fa44e01d08c1fc5ba1f375a2478f5a01580d3`

### Build

```bash
cd programs/offchart-lite
cargo build-sbf --manifest-path Cargo.toml -- --locked
ls -l target/deploy/offchart_lite.so
```

or reproducibly, in the pinned container:

```bash
solana-verify build --library-name offchart_lite \
  --base-image solanafoundation/solana-verifiable-build@sha256:f71be5ca7620b7e40933b7f1294fa44e01d08c1fc5ba1f375a2478f5a01580d3
```

### Compare with mainnet

Compute the hashes yourself every time; this README intentionally does not hard-code a "current" hash.

```bash
# hash of your build (the executable bytes)
solana-verify get-executable-hash target/deploy/offchart_lite.so
# hash of the program that is deployed right now
solana-verify -u https://api.mainnet-beta.solana.com get-program-hash 8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P
# verify the repository commit against the live program
solana-verify verify-from-repo -u https://api.mainnet-beta.solana.com \
  --program-id 8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P \
  https://github.com/sipLinchik/offchart-program \
  --commit-hash <COMMIT> --library-name offchart_lite --mount-path programs/offchart-lite
```

The OtterSec verification status for the program can be read at
`https://verify.osec.io/status/8vbugQ1PoVv3f3c5X1Np1K4s56juC72VMSKhtBp9bo2P`.

Notes on what the hashes mean:

- `get-executable-hash` hashes the executable bytes only. The **ProgramData** account keeps its original allocation after an upgrade and
  a shorter binary is followed by zero padding, so a raw SHA-256 of the whole ProgramData account (or of a dump padded to the allocated
  length) is **not** the same value as the executable hash. Compare executable hashes with executable hashes.
- Earlier versions of the program (a 41 760-byte binary with an allowlist-based design) were hashed in the past; those hashes belong to
  that historical code and do not describe the current source.
- The initial deploy signature above is the signature of the initial deployment, not of any later upgrade.
- The source can be verified against the live program only after the upgrade that deploys it has actually happened; until then a
  mismatch with the on-chain hash is expected.

> **Keep the crate its own workspace.** `programs/offchart-lite/Cargo.toml` contains its own `[workspace]` table. Moving the crate
> into a root workspace produces a different binary, so there is intentionally no root `Cargo.toml`, and the mount path is
> `programs/offchart-lite`.

## Repository layout

```text
.
├── programs/
│   └── offchart-lite/
│       ├── Cargo.toml        # standalone workspace
│       ├── Cargo.lock
│       └── src/
│           ├── lib.rs
│           ├── program.rs    # entrypoint, settle, price and cap checks
│           ├── ops.rs        # config and order instructions
│           └── math.rs       # fee / floor / ceiling arithmetic (host-tested)
├── rust-toolchain.toml       # Rust 1.89.0
├── LICENSE
├── SECURITY.md
└── CONTRIBUTING.md
```

## License

[MIT](LICENSE) © 2026 Offchart
