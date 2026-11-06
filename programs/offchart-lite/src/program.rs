//! offchart-lite — `settle` of the offchart program on pinocchio (no Anchor, no IDL, no events).
//!
//! Byte-compatible with the Anchor program of r8: same 8-byte discriminators, same `Config` / `Order` /
//! `TokenConfig` layouts (offsets below), same PDA seeds, same account order of `settle`, same error numbers
//! (6000 + index of `OffchartError`). The TS layer therefore does not change. See docs/MAINNET-BUDGET.md §2.5.
//!
//! SIZE RULES (every SBF instruction is 8 bytes, and each bounds check / `unwrap` is a panic path):
//!  * account data is viewed as a fixed-size array (`view::<N>`): ONE length check, then constant offsets only;
//!  * no 128-bit division, no `copy_from_slice`/slicing with computed ranges on the hot path;
//!  * account data is read through raw pointers: no reference into account data is ever kept across a CPI, and the
//!    only writes (order bookkeeping) happen after every read, at the very end.
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
const ED25519: Address = a("Ed25519SigVerify111111111111111111111111111");
const IX_SYSVAR: Address = a("Sysvar1nstructions1111111111111111111111111");
const NATIVE: Address = a("So11111111111111111111111111111111111111112");

const DISC_SETTLE: u64 = u64::from_le_bytes([0xaf, 0x2a, 0xb9, 0x57, 0x90, 0x83, 0x66, 0xd4]);
const DISC_CONFIG: u64 = u64::from_le_bytes([0x9b, 0x0c, 0xaa, 0xe0, 0x1e, 0xfa, 0xcc, 0x82]);
const DISC_ORDER: u64 = u64::from_le_bytes([0x86, 0xad, 0xdf, 0xb9, 0x4d, 0x56, 0x1c, 0x33]);
const DISC_TOKEN_CONFIG: u64 = u64::from_le_bytes([0x5c, 0x49, 0xff, 0x2b, 0x6b, 0x33, 0x75, 0x65]);

const PRICE_DOMAIN: [u8; 22] = *b"offchart.pricebatch.v1";
const DOM: usize = 22;
const STRIDE: usize = 10;
const HEAD: usize = 6; // keeper, config, delegate, escrow, instructions, system
const TOKEN_ACCOUNT_LEN: u64 = 165;
/// Rent-exempt minimum of a 165-byte token account: (165 + 128) * 3480 * 2. Hard-coded on purpose: pinocchio's
/// `Rent` reads the SIMD-0194 layout and returns HALF of this on today's sysvar (found by the rM2 tests). If the
/// cluster rate ever changes, `initialize_account3` rejects the temp account and the whole tx fails safely.
const RENT_TOKEN_ACCOUNT: u64 = 2_039_280;
/// rent(len) = (len + 128) * 6960 (3480 lamports/byte-year * 2 years) — same hard-coded rate, same safe-fail.
const fn rent(len: usize) -> u64 {
    (len as u64 + 128) * 6960
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
// TokenConfig (disc 8 + 132)
const T_LEN: usize = 140;
const T_MINT: usize = 8;
const T_STATUS: usize = 40;
const T_VAULT_BASE: usize = 73;
const T_VAULT_QUOTE: usize = 105;
const T_TOLERANCE: usize = 137;
// Order (disc 8 + 281)
const O_LEN: usize = 289;
const O_OWNER: usize = 8;
const O_KIND: usize = 40;
const O_GIVE_MINT: usize = 41;
const O_TOTAL: usize = 73;
const O_FILLED: usize = 81;
const O_WANTS: usize = 89;
const O_WANT_COUNT: usize = 249;
const O_LIMIT_BPS: usize = 250;
const O_ABS_LIMIT: usize = 252;
const O_MIN_FILL: usize = 260;
const O_PARTIAL: usize = 262;
const O_EXPIRES: usize = 263;
const O_STATUS: usize = 271;

// the raw reads below are only sound if every offset lies inside its struct: proven at compile time
const _: () = {
    assert!(C_DELEGATE_BUMP < C_LEN && C_MAX_AGE + 8 <= C_LEN && C_TREASURY + 32 <= C_LEN && C_FEE + 2 <= C_LEN);
    assert!(T_TOLERANCE + 2 <= T_LEN && T_VAULT_QUOTE + 32 <= T_LEN);
    assert!(O_STATUS < O_LEN && O_EXPIRES + 8 <= O_LEN && O_WANTS + 160 == O_WANT_COUNT && O_ABS_LIMIT + 8 == O_MIN_FILL);
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
        let n = run_settle(program_id, accounts, args)?;
        finish(accounts, n, args)
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
    price: u64,
    fee: u64,
    net: u64,
    decimals: u8,
    tmp_bump: u8,
}

fn u16_at(b: &[u8], o: usize) -> Result<usize, ProgramError> {
    Ok(u16::from_le_bytes(*arr::<2>(b, o)?) as usize)
}
fn sl(b: &[u8], o: usize, l: usize) -> Result<&[u8], ProgramError> {
    b.get(o..o + l).ok_or(BAD_BATCH)
}
/// `N` bytes at `o` as an array reference (no panic path).
fn arr<const N: usize>(b: &[u8], o: usize) -> Result<&[u8; N], ProgramError> {
    sl(b, o, N)?.try_into().map_err(|_| BAD_BATCH)
}

/// Parse `PriceBatch` from the ed25519 instruction right before this one. Returns the signed message.
#[cfg_attr(feature = "prof", inline(never))]
fn price_message<'a>(
    sysvar: &'a AccountView,
    oracle: &[u8; 32],
    max_age: u64,
    slot: u64,
    program_id: &Address,
) -> Result<&'a [u8], ProgramError> {
    ensure!(*sysvar.address() == IX_SYSVAR, BAD_BATCH);
    // SAFETY: read-only view of the sysvar account data; nothing writes it during the instruction.
    let s: &[u8] = unsafe { sysvar.borrow_unchecked() };
    ensure!(s.len() >= 4, BAD_BATCH);
    let cur = u16_at(s, s.len() - 2)?;
    ensure!(cur >= 1, BAD_BATCH);
    let off = u16_at(s, 2 + 2 * (cur - 1))?;
    let p = off + 2 + 33 * u16_at(s, off)?;
    ensure!(sl(s, p, 32)? == ED25519.as_array(), BAD_BATCH);
    let ix = sl(s, p + 34, u16_at(s, p + 32)?)?;
    // one signature, everything inline (indices 0xFFFF): the precompile verified (pubkey, message) of THIS data
    ensure!(ix.len() >= 16 && arr::<1>(ix, 0)?[0] == 1, BAD_BATCH);
    ensure!(u16_at(ix, 4)? == 0xFFFF && u16_at(ix, 8)? == 0xFFFF && u16_at(ix, 14)? == 0xFFFF, BAD_BATCH);
    ensure!(sl(ix, u16_at(ix, 6)?, 32)? == oracle, BAD_BATCH);
    let msg = sl(ix, u16_at(ix, 10)?, u16_at(ix, 12)?)?;
    ensure!(*arr::<DOM>(msg, 0)? == PRICE_DOMAIN && arr::<32>(msg, DOM)? == program_id.as_array(), BAD_BATCH);
    let n = arr::<1>(msg, DOM + 40)?[0] as usize;
    ensure!(n <= 16 && msg.len() == DOM + 41 + n * 40, BAD_BATCH);
    let bslot = u64::from_le_bytes(*arr::<8>(msg, DOM + 32)?);
    ensure!(bslot <= slot && slot - bslot <= max_age, STALE);
    Ok(msg)
}

#[cfg_attr(feature = "prof", inline(never))]
fn price_of(msg: &[u8], mint: &Address) -> Result<u64, ProgramError> {
    if *mint == NATIVE {
        return Ok(math::PRICE_SCALE as u64);
    }
    let n = arr::<1>(msg, DOM + 40).map_err(|_| NO_PRICE)?[0] as usize;
    for i in 0..n {
        let o = DOM + 41 + i * 40;
        if arr::<32>(msg, o).map_err(|_| NO_PRICE)? == mint.as_array() {
            let p = u64::from_le_bytes(*arr::<8>(msg, o + 32).map_err(|_| NO_PRICE)?);
            ensure!(p > 0, NO_PRICE);
            return Ok(p);
        }
    }
    Err(NO_PRICE)
}

#[cfg_attr(feature = "prof", inline(never))]
fn run_settle(pid: &Address, accounts: &[AccountView], args: &[u8]) -> Result<usize, ProgramError> {
    // args = borsh Vec<u64>
    let Some((h, _)) = args.split_first_chunk::<4>() else {
        return Err(BAD_ACCOUNTS);
    };
    let n = u32::from_le_bytes(*h) as usize;
    ensure!((n == 2 || n == 3) && args.len() == 4 + 8 * n && accounts.len() == HEAD + STRIDE * n, BAD_ACCOUNTS);
    let [keeper, config_acc, delegate, _escrow, sysvar, system, rest @ ..] = accounts else {
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
    let msg = price_message(sysvar, key!(cfg, C_ORACLE), rd!(cfg, C_MAX_AGE, u64), clock.slot, pid)?;

    let mut ps = [Part::default(); 3];
    // ---------- per participant: order, mint, token config, source, volume, price
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
        // mint: initialized, and (Token-2022) without ANY extension — the length of a bare mint
        ensure!(mint.data_len() == 82, UNSUPPORTED_EXT);
        let m = view::<82>(mint)?;
        ensure!(m[45] == 1, UNSUPPORTED_EXT);
        if !is_native {
            let tc = owned::<T_LEN>(&p[5], pid, DISC_TOKEN_CONFIG, TOKEN_NOT_ALLOWED)?;
            ensure!(key!(tc, T_MINT) == mk.as_array() && tc[T_STATUS] == 0, TOKEN_NOT_ALLOWED);
        }
        ensure!(src.is_writable(), BAD_ACCOUNTS);
        let s = tok(src, tpa, BAD_TOKEN_ACC)?;
        ensure!(key!(s, 0) == mk.as_array() && key!(s, 32) == key!(o, O_OWNER), BAD_TOKEN_ACC);
        ensure!(rd!(s, 72, u32) == 1 && key!(s, 76) == delegate.address().as_array(), BAD_DELEGATE);
        ensure!(rd!(s, 121, u64) >= fill, BAD_DELEGATE);
        ensure!(rd!(s, 64, u64) >= fill, NO_BALANCE);
        let fee = math::fee_of(fill, fee_bps);
        ps[i % 3] = Part { fill, price: price_of(msg, mk)?, fee, net: fill - fee, decimals: m[44], tmp_bump: 0 };
    }
    for i in 1..n {
        for j in 0..i {
            let (pi, pj) = (pacc(rest, i), pacc(rest, j));
            ensure!(pi[0].address() != pj[0].address() && pi[1].address() != pj[1].address(), BAD_ACCOUNTS);
        }
    }

    // ---------- cycle: recipients, fees, price limits, pool reserves
    for i in 0..n {
        let j = (i + 1) % n;
        let (gp, rp) = (pacc(rest, i), pacc(rest, j));
        let (g, r) = (ps[i % 3], ps[j % 3]);
        let ro = view::<O_LEN>(&rp[0])?;
        let gmint = gp[3].address();
        let gtp = gp[4].address();
        let mut wanted = false;
        for k in 0..5 {
            wanted |= k < ro[O_WANT_COUNT] as usize && key!(ro, O_WANTS + 32 * k) == gmint.as_array();
        }
        ensure!(wanted, NOT_WANTED);
        let dest = &rp[2];
        ensure!(dest.is_writable(), BAD_RECIPIENT);
        if *gmint == NATIVE {
            // the receiver gets NATIVE SOL on the wallet, through a temporary wSOL account
            ensure!(dest.address().as_array() == key!(ro, O_OWNER), BAD_RECIPIENT);
            let (tmp, bump) = Address::try_find_program_address(&[b"wsol", rp[0].address().as_array()], pid)
                .ok_or(BAD_ACCOUNTS)?;
            ensure!(tmp == *rp[6].address() && rp[6].is_writable(), BAD_ACCOUNTS);
            ps[i % 3].tmp_bump = bump; // slot i: bump of the temp account used when i pays j
        } else {
            let t = tok(dest, gtp, BAD_RECIPIENT)?;
            ensure!(key!(t, 0) == gmint.as_array() && key!(t, 32) == key!(ro, O_OWNER), BAD_RECIPIENT);
        }
        if g.fee > 0 {
            let tr = &gp[7];
            ensure!(tr.is_writable(), BAD_RECIPIENT);
            let t = tok(tr, gtp, BAD_RECIPIENT)?;
            ensure!(key!(t, 0) == gmint.as_array() && key!(t, 32) == key!(cfg, C_TREASURY), BAD_RECIPIENT);
        }
        // limits of the receiving side j: it gives r.fill (price r.price) and gets g.net (price g.price)
        let (lim, abs) = (rd!(ro, O_LIMIT_BPS, u16), rd!(ro, O_ABS_LIMIT, u64));
        let ok = if ro[O_KIND] == 1 {
            math::buyer_ok(r.fill, r.price, g.net, g.price, lim, abs)
        } else {
            math::seller_ok(r.fill, r.price, g.net, g.price, lim, abs)
        }
        .ok_or(MATH)?;
        ensure!(ok, PRICE_LIMIT);
    }
    for i in 0..n {
        let p = pacc(rest, i);
        if *p[3].address() == NATIVE {
            continue;
        }
        let tc = view::<T_LEN>(&p[5])?;
        if *key!(tc, T_VAULT_BASE) != [0u8; 32] {
            let (vb, vq) = (&p[8], &p[9]);
            ensure!(key!(tc, T_VAULT_BASE) == vb.address().as_array() && key!(tc, T_VAULT_QUOTE) == vq.address().as_array(), BAD_ACCOUNTS);
            let (base, quote) = (tok_any(vb)?, tok_any(vq)?);
            ensure!(key!(base, 0) == p[3].address().as_array() && key!(quote, 0) == NATIVE.as_array(), BAD_TOKEN_ACC);
            let ok = math::pool_ok(ps[i % 3].price, rd!(quote, 64, u64), rd!(base, 64, u64), rd!(tc, T_TOLERANCE, u16)).ok_or(MATH)?;
            ensure!(ok, POOL_DEV);
        }
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
            transfer_checked(tp, src, mint, &gp[7], delegate, g.fee, g.decimals, &dsigner)?;
        }
        if *mint.address() != NATIVE {
            transfer_checked(tp, src, mint, &rp[2], delegate, g.net, g.decimals, &dsigner)?;
            continue;
        }
        // wSOL -> temp wSOL account (owner = delegate PDA) -> closed into the keeper -> native SOL to the wallet
        let (tmp, dest) = (&rp[6], &rp[2]);
        let bump = [g.tmp_bump];
        let tmp_seed = [Seed::from(b"wsol"), Seed::from(rp[0].address().as_array()), Seed::from(&bump)];
        make_account(keeper, tmp, RENT_TOKEN_ACCOUNT, TOKEN_ACCOUNT_LEN, tp.address(), &[Signer::from(&tmp_seed)])?;
        let init = build::<33>(&[&[18], delegate.address().as_array()]);
        call(tp.address(), [tmp, mint], [W, 0], &init, &[])?;
        transfer_checked(tp, src, mint, tmp, delegate, g.net, g.decimals, &dsigner)?;
        call(tp.address(), [tmp, keeper, delegate], [W, W, S], &[9], &dsigner)?;
        let pay = build::<12>(&[&[2, 0, 0, 0], &g.net.to_le_bytes()]);
        call(&SYSTEM, [keeper, dest], [W | S, W], &pay, &[])?;
    }
    Ok(n)
}

/// Order bookkeeping — the only writes of the instruction. Fills were validated in `run_settle`.
#[cfg_attr(feature = "prof", inline(never))]
fn finish(accounts: &mut [AccountView], n: usize, args: &[u8]) -> ProgramResult {
    for (i, oa) in accounts.iter_mut().skip(HEAD).step_by(STRIDE).take(n).enumerate() {
        // SAFETY: all reads are done, no other reference to this account's data is alive; length checked in run_settle.
        let o = unsafe { &mut *(oa.data_mut_ptr() as *mut [u8; O_LEN]) };
        let filled = rd!(o, O_FILLED, u64).checked_add(rd!(args, 4 + 8 * i, u64)).ok_or(MATH)?;
        unsafe { core::ptr::write_unaligned(o.as_mut_ptr().add(O_FILLED) as *mut u64, filled) };
        o[O_STATUS] = if filled == rd!(o, O_TOTAL, u64) { 2 } else { 1 };
    }
    Ok(())
}
