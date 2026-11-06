//! offchart-lite — `settle` of the offchart program on pinocchio (no Anchor, no IDL, no events).
//!
//! Byte-compatible with the Anchor program of r8: same 8-byte discriminators, same `Config` / `Order` /
//! `TokenConfig` layouts, same PDA seeds, same account order of `settle`, same error numbers (6000 + index of
//! `OffchartError`). The TS layer therefore does not change. See docs/MAINNET-BUDGET.md and DECISIONS D-rM2-*.
//!
//! `math` is plain Rust (host-testable); `program` exists for the SBF target only.
#![cfg_attr(target_os = "solana", no_std)]

pub mod math;
#[cfg(target_os = "solana")]
mod program;
