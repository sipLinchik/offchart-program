//! offchart-lite — the Offchart on-chain program on pinocchio (no Anchor, no IDL, no events): settle + order lifecycle + config.
//!
//! Any compatible SPL / Token-2022 mint trades with NO registration: there is no TokenConfig, no pool vault, no signed price batch and no liquidity
//! gate. An order carries the owner's own NET floors in raw units (`min_outs`); `settle` (keeper-signed, 2-3 participants) checks accounts,
//! delegation, balances, mint extensions, the 0.5 % fee and every receiver's floor, plus the rCap rules:
//!  * a cap on the cumulative USD value of ONE order (default $100, `Config` limits record), measured ONLY on an anchor leg (wSOL / USDC / USDT)
//!    from the pinned canonical Pyth push-feed accounts (price + confidence, rounded up) — unknown <-> unknown is refused;
//!  * the limits record in `Config.oracle` carries a format marker; a pre-rCap Config is refused (6043), never reinterpreted.
//! Account layouts (`Config` 150 B, `Order` 329 B), discriminators, PDA seeds and error numbers 6000+ keep the r8 numbering; rCap adds 6040-6043 and
//! reuses two former slots (Config.oracle = limits record, Config.max_price_age_slots = price freshness in SECONDS, Order bytes 252..260 = value used).
//! `settle` accounts: 4 head + 7 per participant + 0..3 Pyth price accounts. See docs/RCAP-ONCHAIN-CAP.md and docs/ANY-TOKEN-ARCHITECTURE.md.
//!
//! SIZE RULES (every SBF instruction is 8 bytes, and each bounds check / `unwrap` is a panic path):
//!  * account data is viewed as a fixed-size array (`view::<N>`): ONE length check, then constant offsets only;
//!  * no 128-bit division, no `copy_from_slice`/slicing with computed ranges on the hot path;
//!  * account data is read through raw pointers: no reference into account data is ever kept across a CPI, and the
//!    only writes (order bookkeeping) happen after every read, at the very end.
#![allow(dead_code)]
use crate::math;
use pinocchio::{
    cpi::{invoke_signed, Seed, Signer},
    error::ProgramError,
    instruction::{InstructionAccount, InstructionView},
    sysvars::{clock::Clock, Sysvar},
    AccountView, Address, ProgramResult,
};
use solana_address::Address as Addr;

pinocchio::program_entrypoint!(process, 36);
pinocchio::no_allocator!();
pinocchio::nostd_panic_handler!();

// ------------------------------------------------------------------ constants
const fn a(s: &str) -> Address {
    Addr::from_str_const(s)
}
const TOKEN: Address = a("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const TOKEN22: Address = a("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
const SYSTEM: Address = Address::new_from_array([0u8; 32]);
const NATIVE: Address = a("So11111111111111111111111111111111111111112");

/// rCap: independent canonical price source = Pyth `PriceUpdateV2` accounts of the Pyth Solana receiver (docs/RCAP-ONCHAIN-CAP.md).
const PYTH: Address = a("rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ");
const DISC_PRICE: u64 = u64::from_le_bytes([0x22, 0xf1, 0x23, 0x63, 0x9d, 0x7e, 0xf4, 0xcd]);
const fn hx(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    let mut o = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        let (h, l) = (b[2 * i], b[2 * i + 1]);
        let hv = if h >= b'a' { h - b'a' + 10 } else { h - b'0' };
        let lv = if l >= b'a' { l - b'a' + 10 } else { l - b'0' };
        o[i] = hv * 16 + lv;
        i += 1;
    }
    o
}
/// The only mints with an independent USD price: wSOL, USDC, USDT (addresses are constants, NOT a registry). Anything else is "unknown".
const ANCHOR_MINTS: [Address; 3] = [NATIVE, a("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"), a("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB")];
const ANCHOR_FEEDS: [[u8; 32]; 3] = [
    hx("ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d"),
    hx("eaa020c61cc479712813461ce153894a96a6c00b21ed0cfc2798d1f9a9e9c94a"),
    hx("2b89b9dc8fdf9f34709a5b106b472f0f39bb6ca9ce04b0fd7f2e971688e2e53b"),
];
const ANCHOR_DEC: [u8; 3] = [9, 6, 6];
/// The CANONICAL sponsored push-feed accounts (Pyth push oracle `pythWSnswVUd12oZpeFP8e9CVaEqJg25g1Vtc2biRsT`, shard 0, seeds [0u16 LE, feed id]).
/// Pinned by ADDRESS: a caller cannot pick another (older/cheaper) receiver-owned update. Verified by tests/rcap.rs and packages/chain anchors.test.ts.
const ANCHOR_PRICE: [Address; 3] = [
    a("7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE"),
    a("Dpw1EAVrSB1ibxiDQyTAW6Zip3J4Btk2x4SgApQCeFbX"),
    a("HT2PLQBcG5EiCcNSaMHAjSgd9F98ecpATbk4Sk5oYuM"),
];
/// 10^k, k = 0..=15
const POW10: [u64; 16] = {
    let mut t = [1u64; 16];
    let mut i = 1;
    while i < 16 {
        t[i] = t[i - 1] * 10;
        i += 1;
    }
    t
};
const MAX_CONF_BPS: u128 = 100; // conf must be <= 1 % of price
const MAX_FUTURE_S: i64 = 10;
const O_VALUE_USED: usize = 252; // (was abs_limit, reserved) cumulative USD value of this order's fills, 1e-6 USD, upper bound

const DISC_SETTLE: u64 = u64::from_le_bytes([0xaf, 0x2a, 0xb9, 0x57, 0x90, 0x83, 0x66, 0xd4]);
const DISC_CONFIG: u64 = u64::from_le_bytes([0x9b, 0x0c, 0xaa, 0xe0, 0x1e, 0xfa, 0xcc, 0x82]);
const DISC_ORDER: u64 = u64::from_le_bytes([0x86, 0xad, 0xdf, 0xb9, 0x4d, 0x56, 0x1c, 0x33]);

/// rAny: no oracle, no TokenConfig, no pool vaults. Participant accounts:
/// order, source, destination, mint, token program, temp-wSOL, treasury ATA.
const STRIDE: usize = 7;
const HEAD: usize = 4; // keeper, config, delegate, system
/// Token-2022 mint extensions that are safe for `transfer_checked` of raw amounts (bit = ExtensionType id):
/// MintCloseAuthority 3, DefaultAccountState 6, InterestBearing 10, MetadataPointer 18, TokenMetadata 19, GroupPointer 20,
/// TokenGroup 21, GroupMemberPointer 22, TokenGroupMember 23, ScaledUiAmount 25, Pausable 26. Everything else (transfer fee,
/// hook, permanent delegate, non-transferable, confidential, unknown) is refused. docs/ANY-TOKEN-ARCHITECTURE.md §4.
const EXT_OK: u32 = (1 << 3) | (1 << 6) | (1 << 10) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23) | (1 << 25) | (1 << 26);
const TOKEN_ACCOUNT_LEN: u64 = 165;
/// Rent-exempt minimum of `len` data bytes, read from the Rent sysvar (rAnyR, D-rAnyR-02). The sysvar layout is
/// `lamports_per_byte: u64` + `exemption_threshold: f64` + `burn_percent: u8`; pinocchio's `Rent` reads only the first
/// u64 (SIMD-0194 layout) and would return HALF of the true minimum on a cluster that still carries threshold 2.0
/// (found by the rM2 tests). So both fields are read here: the f64 is compared by BIT PATTERN (no soft-float code) and only
/// the two values that exist in the wild are accepted — 1.0 (post SIMD-0194) and 2.0 (legacy). Anything else fails
/// closed (the tx reverts), never a guess. Mainnet 07.11: 5080 lamports/byte => 165 B = 1 488 440, 329 B = 2 321 560.
fn rent(len: usize) -> Result<u64, ProgramError> {
    let (mut lo, mut hi) = ([0u8; 8], [0u8; 8]);
    pinocchio::sysvars::get_sysvar(&mut lo, &pinocchio::sysvars::rent::RENT_ID, 0)?;
    pinocchio::sysvars::get_sysvar(&mut hi, &pinocchio::sysvars::rent::RENT_ID, 8)?;
    let mult: u64 = match u64::from_le_bytes(hi) {
        0x3FF0_0000_0000_0000 => 1,
        0x4000_0000_0000_0000 => 2,
        _ => return Err(BAD_ACCOUNTS),
    };
    u64::from_le_bytes(lo).checked_mul(mult).and_then(|r| r.checked_mul(len as u64 + 128)).ok_or(MATH)
}

// Config (disc 8 + 142)
const C_LEN: usize = 150;
const C_ADMIN: usize = 8;
const C_KEEPER: usize = 40;
const C_ORACLE: usize = 72;
const C_TREASURY: usize = 104;
const C_FEE: usize = 136;
const C_PAUSED: usize = 138;
const C_MAX_AGE: usize = 139;
const C_DELEGATE_BUMP: usize = 148;
// Order (disc 8 + 281 + rAny: min_out[5] = 40)
const O_LEN: usize = 329;
const O_MINOUT: usize = 289;
const O_OWNER: usize = 8;
const O_KIND: usize = 40;
const O_GIVE_MINT: usize = 41;
const O_TOTAL: usize = 73;
const O_FILLED: usize = 81;
const O_WANTS: usize = 89;
const O_WANT_COUNT: usize = 249;
const O_MIN_FILL: usize = 260;
const O_PARTIAL: usize = 262;
const O_EXPIRES: usize = 263;
const O_STATUS: usize = 271;

// the raw reads below are only sound if every offset lies inside its struct: proven at compile time
const _: () = {
    assert!(C_DELEGATE_BUMP < C_LEN && C_MAX_AGE + 8 <= C_LEN && C_TREASURY + 32 <= C_LEN && C_FEE + 2 <= C_LEN);
    assert!(O_STATUS < O_LEN && O_EXPIRES + 8 <= O_LEN && O_WANTS + 160 == O_WANT_COUNT && O_MINOUT + 40 == O_LEN);
};

// ------------------------------------------------------------------ errors (= Anchor numbers 6000+)
const fn err(i: u32) -> ProgramError {
    ProgramError::Custom(6000 + i)
}
const NOT_KEEPER: ProgramError = err(1);
const PAUSED: ProgramError = err(2);
const MATH: ProgramError = err(5);
const EXPIRED: ProgramError = err(6);
const NOT_ACTIVE: ProgramError = err(7);
#[allow(dead_code)]
const TOKEN_NOT_ALLOWED: ProgramError = err(8);
const UNSUPPORTED_EXT: ProgramError = err(9);
const BAD_BATCH: ProgramError = err(10);
const STALE: ProgramError = err(11);
const NO_PRICE: ProgramError = err(12);
const BAD_FILL: ProgramError = err(13);
const PRICE_LIMIT: ProgramError = err(14);
const POOL_DEV: ProgramError = err(15);
const BAD_RECIPIENT: ProgramError = err(16);
const NOT_WANTED: ProgramError = err(17);
const BAD_ACCOUNTS: ProgramError = err(18);
const BAD_DELEGATE: ProgramError = err(19);
const NO_BALANCE: ProgramError = err(20);
const BAD_TOKEN_ACC: ProgramError = err(21);
const BAD_TOKEN_PROG: ProgramError = err(22);
const CAP_EXCEEDED: ProgramError = err(40);
const UNPRICED: ProgramError = err(41);
const BAD_ORACLE: ProgramError = err(42);
const CONFIG_OLD: ProgramError = err(43); // Config still has the pre-rCap `oracle` key where the cap belongs: set_config first

macro_rules! ensure {
    ($c:expr, $e:expr) => {
        if !($c) {
            return Err($e);
        }
    };
}

// ------------------------------------------------------------------ raw readers (offsets are compile-time consts)
/// `$t` at byte offset `$o` of the fixed-size array `$b` (little-endian target, unaligned read).
macro_rules! rd {
    ($b:expr, $o:expr, $t:ty) => {
        // SAFETY: `$b` is a `&[u8; N]` view and `$o + size_of::<$t>() <= N` (const-asserted for the struct offsets).
        unsafe { core::ptr::read_unaligned($b.as_ptr().add($o) as *const $t) }
    };
}
/// 32 bytes at offset `$o` as `&[u8; 32]`.
macro_rules! key {
    ($b:expr, $o:expr) => {
        // SAFETY: as for `rd!`.
        unsafe { &*($b.as_ptr().add($o) as *const [u8; 32]) }
    };
}

/// Account data as a fixed-size array: ONE length check, constant-offset reads afterwards.
#[inline(always)]
fn view<const N: usize>(acc: &AccountView) -> Result<&[u8; N], ProgramError> {
    ensure!(acc.data_len() >= N, BAD_ACCOUNTS);
    // SAFETY: data_len >= N; u8 has alignment 1; no mutable alias exists while this view is alive (crate docs).
    Ok(unsafe { &*(acc.data_ptr() as *const [u8; N]) })
}

/// A program-owned account of type `disc` (owner + discriminator + size).
#[inline(always)]
fn owned<'a, const N: usize>(acc: &'a AccountView, pid: &Address, disc: u64, e: ProgramError) -> Result<&'a [u8; N], ProgramError> {
    ensure!(acc.owned_by(pid), e);
    let Ok(b) = view::<N>(acc) else { return Err(e) };
    ensure!(rd!(b, 0, u64) == disc, e);
    Ok(b)
}

/// Token account of `tp` in state Initialized (not frozen).
fn tok<'a>(acc: &'a AccountView, tp: &Address, e: ProgramError) -> Result<&'a [u8; 165], ProgramError> {
    ensure!(acc.owned_by(tp), e);
    let Ok(t) = view::<165>(acc) else { return Err(e) };
    ensure!(t[108] == 1, e);
    Ok(t)
}

fn tok_any(acc: &AccountView) -> Result<&[u8; 165], ProgramError> {
    tok(acc, if acc.owned_by(&TOKEN) { &TOKEN } else { &TOKEN22 }, BAD_TOKEN_ACC)
}

/// Participant `i` of `settle`: its 10 consecutive accounts.
#[inline(always)]
fn pacc(rest: &[AccountView], i: usize) -> &[AccountView; STRIDE] {
    // SAFETY: the caller checked `rest.len() == n * STRIDE` and `i < n`; AccountView is a plain pointer wrapper.
    unsafe { &*(rest.as_ptr().add(i * STRIDE) as *const [AccountView; STRIDE]) }
}

// ------------------------------------------------------------------ CPI
const W: u8 = 1;
const S: u8 = 2;

/// Concatenation of `parts` into a fixed-size array without any slicing/bounds-check panic path.
/// Contract (every call site, covered by the CPI tests): the parts' lengths add up to exactly `N`.
fn build<const N: usize>(parts: &[&[u8]]) -> [u8; N] {
    let mut d = [0u8; N];
    let mut o = 0usize;
    for p in parts {
        // SAFETY: o + p.len() <= N by the contract above.
        unsafe { core::ptr::copy_nonoverlapping(p.as_ptr(), d.as_mut_ptr().add(o), p.len()) };
        o += p.len();
    }
    d
}

fn call<const N: usize>(
    prog: &Address,
    accs: [&AccountView; N],
    flags: [u8; N],
    data: &[u8],
    signers: &[Signer],
) -> ProgramResult {
    let metas: [InstructionAccount; N] =
        core::array::from_fn(|i| InstructionAccount::new(accs[i].address(), flags[i] & W != 0, flags[i] & S != 0));
    let ix = InstructionView { program_id: prog, data, accounts: &metas };
    invoke_signed(&ix, &accs, signers)
}

#[cfg_attr(feature = "prof", inline(never))]
fn transfer_checked(
    tp: &AccountView,
    from: &AccountView,
    mint: &AccountView,
    to: &AccountView,
    auth: &AccountView,
    amount: u64,
    decimals: u8,
    signers: &[Signer],
) -> ProgramResult {
    let data = build::<10>(&[&[12], &amount.to_le_bytes(), &[decimals]]);
    call(tp.address(), [from, mint, to, auth], [W, 0, W, S], &data, signers)
}

/// Creates (or takes over a griefed, pre-funded) system account as `owner`'s PDA-style account: top-up to `need`
/// lamports, `allocate`, `assign`. Works on empty accounts of any balance, unlike `create_account`.
#[cfg_attr(feature = "prof", inline(never))]
fn make_account(
    payer: &AccountView,
    new: &AccountView,
    need: u64,
    len: u64,
    owner: &Address,
    signers: &[Signer],
) -> ProgramResult {
    let have = new.lamports();
    if have < need {
        let t = build::<12>(&[&[2, 0, 0, 0], &(need - have).to_le_bytes()]);
        call(&SYSTEM, [payer, new], [W | S, W], &t, &[])?;
    }
    let al = build::<12>(&[&[8, 0, 0, 0], &len.to_le_bytes()]);
    call(&SYSTEM, [new], [W | S], &al, signers)?;
    let asg = build::<36>(&[&[1, 0, 0, 0], owner.as_array()]);
    call(&SYSTEM, [new], [W | S], &asg, signers)
}

// ------------------------------------------------------------------ entry
pub fn process(program_id: &Address, accounts: &mut [AccountView], data: &[u8]) -> ProgramResult {
    let Some((disc, args)) = data.split_first_chunk::<8>() else {
        return Err(ProgramError::InvalidInstructionData);
    };
    if u64::from_le_bytes(*disc) == DISC_SETTLE {
        let (n, used) = run_settle(program_id, accounts, args)?;
        finish(accounts, n, args, used)
    } else {
        ops::dispatch(program_id, accounts, u64::from_le_bytes(*disc), args)
    }
}

/// Admin + order-lifecycle instructions (everything except `settle`).
#[path = "ops.rs"]
mod ops;

#[derive(Clone, Copy, Default)]
struct Part {
    fill: u64,
    ai: u8,
    fee: u64,
    net: u64,
    decimals: u8,
    tmp_bump: u8,
}

/// Mint check (create_order and settle). SPL Token: an initialized mint of exactly 82 bytes. Token-2022: a bare mint, or a mint
/// whose TLV extensions are ALL in `EXT_OK` (unknown types are refused: fail-closed). No registry, no admin: the chain is the registry.
#[cfg_attr(feature = "prof", inline(never))]
fn mint_ok(acc: &AccountView) -> ProgramResult {
    let len = acc.data_len();
    let m = view::<82>(acc).map_err(|_| UNSUPPORTED_EXT)?;
    ensure!(m[45] == 1, UNSUPPORTED_EXT);
    if len == 82 {
        return Ok(());
    }
    ensure!(acc.owned_by(&TOKEN22) && len > 166, UNSUPPORTED_EXT);
    // SAFETY: read-only view; nothing writes the mint during the instruction.
    let d: &[u8] = unsafe { acc.borrow_unchecked() };
    ensure!(d.get(165) == Some(&1), UNSUPPORTED_EXT); // AccountType::Mint
    let mut o = 166usize;
    while o < len {
        let Some(h) = d.get(o..).and_then(|x| x.first_chunk::<4>()) else { return Err(UNSUPPORTED_EXT) };
        let t = u16::from_le_bytes([h[0], h[1]]) as u32;
        if t == 0 {
            break; // uninitialised padding
        }
        ensure!(t < 32 && (EXT_OK >> t) & 1 == 1, UNSUPPORTED_EXT);
        o += 4 + u16::from_le_bytes([h[2], h[3]]) as usize;
    }
    Ok(())
}

/// Index of `mint` among the anchors (3 = unknown).
#[inline(always)]
fn anchor_idx(mint: &Address) -> usize {
    let mut k = 3usize;
    for x in 0..3 {
        if *mint == ANCHOR_MINTS[x] {
            k = x;
        }
    }
    k
}

/// Validates one Pyth price-update account and returns (anchor index, upper-confidence price, exponent).
/// Identity: the account ADDRESS must be the pinned canonical push-feed PDA of an anchor, owner = Pyth receiver, not executable, exactly the
/// 134-byte PriceUpdateV2 shape, discriminator, verification Full, `feed_id` equal to the anchor's, `write_authority` = the PDA itself,
/// `posted_slot <= Clock.slot`; then sanity: price > 0, expo in [-12, 0], publish_time fresh and not from the future, conf <= 1 %.
#[cfg_attr(feature = "prof", inline(never))]
fn read_price(acc: &AccountView, now: i64, slot: u64, max_age: i64) -> Result<(usize, u64, i32), ProgramError> {
    ensure!(acc.owned_by(&PYTH) && !acc.executable() && acc.data_len() == 134, BAD_ORACLE);
    let Ok(b) = view::<134>(acc) else { return Err(BAD_ORACLE) };
    ensure!(rd!(b, 0, u64) == DISC_PRICE && b[40] == 1, BAD_ORACLE); // 1 = VerificationLevel::Full
    let mut k = 3usize;
    for x in 0..3 {
        if key!(b, 41) == &ANCHOR_FEEDS[x] && *acc.address() == ANCHOR_PRICE[x] {
            k = x;
        }
    }
    ensure!(k < 3, BAD_ORACLE);
    ensure!(key!(b, 8) == acc.address().as_array() && rd!(b, 125, u64) <= slot, BAD_ORACLE);
    let (price, conf, expo, publish) = (rd!(b, 73, i64), rd!(b, 81, u64), rd!(b, 89, i32), rd!(b, 93, i64));
    ensure!(price > 0 && (-12..=0).contains(&expo), BAD_ORACLE);
    ensure!(publish <= now.saturating_add(MAX_FUTURE_S) && now.saturating_sub(publish) <= max_age, STALE);
    ensure!((conf as u128) * 10_000 <= (price as u128) * MAX_CONF_BPS, BAD_ORACLE);
    let upper = (price as u64).checked_add(conf).ok_or(MATH)?;
    Ok((k, upper, expo))
}

/// ceil(num / 10^sh), sh < 16, WITHOUT a 128-bit division (that alone is ~10 KB of SBF): 10^sh = 2^sh * 5^sh, the power of two is a
/// shift and 5^sh < 2^35 is divided out in 16-bit limbs (remainder < 2^35, so `rem << 16 | limb` < 2^51 never overflows a u64).
/// ceil(ceil(n / a) / b) == ceil(n / (a b)) for positive integers, so the two ceilings compose exactly. Mirrored and fuzzed in tests/math.rs.
pub(crate) fn ceil_div_pow10(num: u128, sh: usize) -> u128 {
    crate::math::ceil_div_pow10(num, sh)
}

/// USD value (1e-6 USD, rounded UP, upper-confidence price) of `amount` raw units of anchor `k`.
#[cfg_attr(feature = "prof", inline(never))]
fn usd_value(px: &[(u64, i32); 3], k: usize, amount: u64) -> Result<u64, ProgramError> {
    let (upper, expo) = px[k % 3];
    ensure!(upper > 0, NO_PRICE);
    // value = amount * upper * 10^(expo + 6 - dec); expo <= 0, dec >= 6 => the exponent is <= 0 and >= -15
    let sh = (ANCHOR_DEC[k % 3] as i32) - 6 - expo;
    ensure!((0..16).contains(&sh), BAD_ORACLE);
    let num = (amount as u128) * (upper as u128); // < 2^128: both factors < 2^64
    let v = ceil_div_pow10(num, (sh as usize) % 16);
    u64::try_from(v).map_err(|_| CAP_EXCEEDED)
}

#[cfg_attr(feature = "prof", inline(never))]
fn run_settle(pid: &Address, accounts: &[AccountView], args: &[u8]) -> Result<(usize, [u64; 3]), ProgramError> {
    // args = borsh Vec<u64>
    let Some((h, _)) = args.split_first_chunk::<4>() else {
        return Err(BAD_ACCOUNTS);
    };
    let n = u32::from_le_bytes(*h) as usize;
    ensure!((n == 2 || n == 3) && args.len() == 4 + 8 * n && accounts.len() >= HEAD + STRIDE * n && accounts.len() <= HEAD + STRIDE * n + 3, BAD_ACCOUNTS);
    let [keeper, config_acc, delegate, system, rest @ ..] = accounts else {
        return Err(BAD_ACCOUNTS);
    };
    let cfg = owned::<C_LEN>(config_acc, pid, DISC_CONFIG, BAD_ACCOUNTS)?;
    ensure!(keeper.is_signer() && key!(cfg, C_KEEPER) == keeper.address().as_array(), NOT_KEEPER);
    ensure!(cfg[C_PAUSED] == 0, PAUSED);
    let fee_bps = rd!(cfg, C_FEE, u16);
    let dbump = [cfg[C_DELEGATE_BUMP]];
    let dpda = Address::create_program_address(&[b"delegate", &dbump], pid).map_err(|_| BAD_ACCOUNTS)?;
    ensure!(dpda == *delegate.address() && *system.address() == SYSTEM, BAD_ACCOUNTS);

    let clock = Clock::get()?;
    // rCap: independent prices of the anchors that were supplied after the participants' accounts
    let max_age = (rd!(cfg, C_MAX_AGE, u64) as i64).min(300);
    let mut px = [(0u64, 0i32); 3];
    for acc in accounts.iter().skip(HEAD + STRIDE * n) {
        let (k, upper, expo) = read_price(acc, clock.unix_timestamp, clock.slot, max_age)?;
        ensure!(px[k % 3].0 == 0, BAD_ORACLE); // each feed at most once
        px[k % 3] = (upper, expo);
    }

    let mut ps = [Part::default(); 3];
    // ---------- per participant: order, mint, source, volume
    for i in 0..n {
        let p = pacc(rest, i);
        let (oa, src, mint, tp) = (&p[0], &p[1], &p[3], &p[4]);
        let o = owned::<O_LEN>(oa, pid, DISC_ORDER, BAD_ACCOUNTS)?;
        ensure!(oa.is_writable(), BAD_ACCOUNTS);
        ensure!(o[O_STATUS] != 2, NOT_ACTIVE);
        ensure!(clock.unix_timestamp < rd!(o, O_EXPIRES, i64), EXPIRED);
        let fill = rd!(args, 4 + 8 * i, u64);
        ensure!(
            math::fill_ok(fill, rd!(o, O_TOTAL, u64), rd!(o, O_FILLED, u64), rd!(o, O_MIN_FILL, u16), o[O_PARTIAL] != 0),
            BAD_FILL
        );
        let mk = mint.address();
        ensure!(key!(o, O_GIVE_MINT) == mk.as_array(), BAD_ACCOUNTS);
        let tpa = tp.address();
        ensure!((*tpa == TOKEN || *tpa == TOKEN22) && mint.owned_by(tpa), BAD_TOKEN_PROG);
        let is_native = *mk == NATIVE;
        ensure!(!is_native || *tpa == TOKEN, BAD_TOKEN_PROG);
        mint_ok(mint)?;
        let m = view::<82>(mint)?;
        let ai = anchor_idx(mk);
        ensure!(ai == 3 || (m[44] == ANCHOR_DEC[ai % 3] && *tpa == TOKEN), BAD_ORACLE); // an anchor is the canonical SPL mint with its known decimals
        ensure!(src.is_writable(), BAD_ACCOUNTS);
        let s = tok(src, tpa, BAD_TOKEN_ACC)?;
        ensure!(key!(s, 0) == mk.as_array() && key!(s, 32) == key!(o, O_OWNER), BAD_TOKEN_ACC);
        ensure!(rd!(s, 72, u32) == 1 && key!(s, 76) == delegate.address().as_array(), BAD_DELEGATE);
        ensure!(rd!(s, 121, u64) >= fill, BAD_DELEGATE);
        ensure!(rd!(s, 64, u64) >= fill, NO_BALANCE);
        let fee = math::fee_of(fill, fee_bps);
        ps[i % 3] = Part { fill, ai: anchor_idx(mk) as u8, fee, net: fill - fee, decimals: m[44], tmp_bump: 0 };
    }
    for i in 1..n {
        for j in 0..i {
            let (pi, pj) = (pacc(rest, i), pacc(rest, j));
            ensure!(pi[0].address() != pj[0].address() && pi[1].address() != pj[1].address(), BAD_ACCOUNTS);
        }
    }

    // ---------- cycle: recipients, fees, the receivers' own floors
    for i in 0..n {
        let j = (i + 1) % n;
        let (gp, rp) = (pacc(rest, i), pacc(rest, j));
        let (g, r) = (ps[i % 3], ps[j % 3]);
        let ro = view::<O_LEN>(&rp[0])?;
        let gmint = gp[3].address();
        let gtp = gp[4].address();
        let mut k = 5usize;
        for x in 0..5 {
            if x < ro[O_WANT_COUNT] as usize && key!(ro, O_WANTS + 32 * x) == gmint.as_array() {
                k = x;
            }
        }
        ensure!(k < 5, NOT_WANTED);
        let dest = &rp[2];
        ensure!(dest.is_writable(), BAD_RECIPIENT);
        if *gmint == NATIVE {
            // the receiver gets NATIVE SOL on the wallet, through a temporary wSOL account
            ensure!(dest.address().as_array() == key!(ro, O_OWNER), BAD_RECIPIENT);
            let (tmp, bump) = Address::try_find_program_address(&[b"wsol", rp[0].address().as_array()], pid)
                .ok_or(BAD_ACCOUNTS)?;
            ensure!(tmp == *rp[5].address() && rp[5].is_writable(), BAD_ACCOUNTS);
            ps[i % 3].tmp_bump = bump; // slot i: bump of the temp account used when i pays j
        } else {
            let t = tok(dest, gtp, BAD_RECIPIENT)?;
            ensure!(key!(t, 0) == gmint.as_array() && key!(t, 32) == key!(ro, O_OWNER), BAD_RECIPIENT);
        }
        if g.fee > 0 {
            let tr = &gp[6];
            ensure!(tr.is_writable(), BAD_RECIPIENT);
            let t = tok(tr, gtp, BAD_RECIPIENT)?;
            ensure!(key!(t, 0) == gmint.as_array() && key!(t, 32) == key!(cfg, C_TREASURY), BAD_RECIPIENT);
        }
        // the receiving side j signed: "for my whole `total` I want at least `min_out`". It gives r.fill and gets g.net:
        // g.net / r.fill >= min_out / total  (u128, no division, no overflow: both factors < 2^64)
        ensure!(math::floor_ok(g.net, rd!(ro, O_TOTAL, u64), rd!(ro, O_MINOUT + 8 * k, u64), r.fill), PRICE_LIMIT);
    }

    // ---------- rCap: USD cap of ONE ORDER, cumulative over all its fills. Value comes only from an independent anchor leg:
    // what the participant gives (if an anchor) or what it receives (if the previous participant gives an anchor); the larger of the
    // two, priced at price+conf, rounded up. A participant with no anchor on either side (unknown <-> unknown) cannot trade.
    ensure!(ops::limits_ok(key!(cfg, C_ORACLE)), CONFIG_OLD);
    let cap = rd!(cfg, C_ORACLE, u64);
    let mut used = [0u64; 3];
    for i in 0..n {
        let prev = (i + n - 1) % n;
        let (gi, gp) = (ps[i % 3].ai as usize, ps[prev % 3].ai as usize);
        ensure!(gi < 3 || gp < 3, UNPRICED);
        let mut v = 0u64;
        if gi < 3 {
            v = usd_value(&px, gi, ps[i % 3].fill)?;
        }
        if gp < 3 {
            v = v.max(usd_value(&px, gp, ps[prev % 3].fill)?);
        }
        let o = view::<O_LEN>(&pacc(rest, i)[0])?;
        let total_used = rd!(o, O_VALUE_USED, u64).checked_add(v).ok_or(CAP_EXCEEDED)?;
        ensure!(total_used <= cap, CAP_EXCEEDED);
        used[i % 3] = total_used;
    }

    // ---------- execute (all or nothing: any error reverts the whole transaction)
    let delegate_seed = [Seed::from(b"delegate"), Seed::from(&dbump)];
    let dsigner = [Signer::from(&delegate_seed)];
    for i in 0..n {
        let j = (i + 1) % n;
        let (gp, rp) = (pacc(rest, i), pacc(rest, j));
        let g = ps[i % 3];
        let (tp, src, mint) = (&gp[4], &gp[1], &gp[3]);
        if g.fee > 0 {
            transfer_checked(tp, src, mint, &gp[6], delegate, g.fee, g.decimals, &dsigner)?;
        }
        if *mint.address() != NATIVE {
            transfer_checked(tp, src, mint, &rp[2], delegate, g.net, g.decimals, &dsigner)?;
            continue;
        }
        // wSOL -> temp wSOL account (owner = delegate PDA) -> closed into the keeper -> native SOL to the wallet
        let (tmp, dest) = (&rp[5], &rp[2]);
        let bump = [g.tmp_bump];
        let tmp_seed = [Seed::from(b"wsol"), Seed::from(rp[0].address().as_array()), Seed::from(&bump)];
        make_account(keeper, tmp, rent(TOKEN_ACCOUNT_LEN as usize)?, TOKEN_ACCOUNT_LEN, tp.address(), &[Signer::from(&tmp_seed)])?;
        let init = build::<33>(&[&[18], delegate.address().as_array()]);
        call(tp.address(), [tmp, mint], [W, 0], &init, &[])?;
        transfer_checked(tp, src, mint, tmp, delegate, g.net, g.decimals, &dsigner)?;
        call(tp.address(), [tmp, keeper, delegate], [W, W, S], &[9], &dsigner)?;
        let pay = build::<12>(&[&[2, 0, 0, 0], &g.net.to_le_bytes()]);
        call(&SYSTEM, [keeper, dest], [W | S, W], &pay, &[])?;
    }
    Ok((n, used))
}

/// Order bookkeeping — the only writes of the instruction. Fills were validated in `run_settle`.
#[cfg_attr(feature = "prof", inline(never))]
fn finish(accounts: &mut [AccountView], n: usize, args: &[u8], used: [u64; 3]) -> ProgramResult {
    for (i, oa) in accounts.iter_mut().skip(HEAD).step_by(STRIDE).take(n).enumerate() {
        // SAFETY: all reads are done, no other reference to this account's data is alive; length checked in run_settle.
        let o = unsafe { &mut *(oa.data_mut_ptr() as *mut [u8; O_LEN]) };
        let filled = rd!(o, O_FILLED, u64).checked_add(rd!(args, 4 + 8 * i, u64)).ok_or(MATH)?;
        unsafe { core::ptr::write_unaligned(o.as_mut_ptr().add(O_FILLED) as *mut u64, filled) };
        unsafe { core::ptr::write_unaligned(o.as_mut_ptr().add(O_VALUE_USED) as *mut u64, used[i % 3]) };
        o[O_STATUS] = if filled == rd!(o, O_TOTAL, u64) { 2 } else { 1 };
    }
    Ok(())
}
