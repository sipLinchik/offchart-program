//! Admin and order-lifecycle instructions of the offchart-lite program: init_config, set_config, upsert_token_config,
//! block_token, create_order, cancel_order, keeper_cancel_order (update_order was cut: D-rM3-01). Same discriminators / argument
//! encodings / account orders / error numbers as the Anchor program of r8 (D-rM2-02). Hardened in rM3: see the
//! arithmetic table in qa/reports/rM3.md and DECISIONS D-rM3-*.
use super::*;

const D_INIT_CONFIG: u64 = u64::from_le_bytes([0x17, 0xeb, 0x73, 0xe8, 0xa8, 0x60, 0x01, 0xe7]);
const D_SET_CONFIG: u64 = u64::from_le_bytes([0x6c, 0x9e, 0x9a, 0xaf, 0xd4, 0x62, 0x34, 0x42]);
const D_UPSERT: u64 = u64::from_le_bytes([0x19, 0x10, 0x18, 0x47, 0xf1, 0x43, 0x7b, 0xaa]);
const D_BLOCK: u64 = u64::from_le_bytes([0xb5, 0x72, 0xf1, 0x7a, 0xcc, 0xa5, 0xcb, 0x37]);
const D_CREATE: u64 = u64::from_le_bytes([0x8d, 0x36, 0x25, 0xcf, 0xed, 0xd2, 0xfa, 0xd7]);
const D_CANCEL: u64 = u64::from_le_bytes([0x5f, 0x81, 0xed, 0xf0, 0x08, 0x31, 0xdf, 0x84]);
const D_KCANCEL: u64 = u64::from_le_bytes([0xeb, 0x54, 0x2b, 0xec, 0x2d, 0xbe, 0x3f, 0xbf]);

const NOT_ADMIN: ProgramError = err(0);
const FEE_TOO_HIGH: ProgramError = err(3);
const INVALID_ARG: ProgramError = err(4);
const NOT_OWNER: ProgramError = err(23);
const LOADER_UPGRADEABLE: Address = a("BPFLoaderUpgradeab1e11111111111111111111111");
const BAD_ARGS: ProgramError = ProgramError::InvalidInstructionData;

/// Borsh cursor over the instruction arguments (no panic path: every read goes through `get`).
struct Rd<'a> {
    b: &'a [u8],
}
impl<'a> Rd<'a> {
    fn take<const N: usize>(&mut self) -> Result<&'a [u8; N], ProgramError> {
        let (h, t) = self.b.split_first_chunk::<N>().ok_or(BAD_ARGS)?;
        self.b = t;
        Ok(h)
    }
    fn u8(&mut self) -> Result<u8, ProgramError> {
        Ok(self.take::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, ProgramError> {
        Ok(u16::from_le_bytes(*self.take::<2>()?))
    }
    fn u32(&mut self) -> Result<u32, ProgramError> {
        Ok(u32::from_le_bytes(*self.take::<4>()?))
    }
    fn u64(&mut self) -> Result<u64, ProgramError> {
        Ok(u64::from_le_bytes(*self.take::<8>()?))
    }
    /// borsh `bool`: 0 or 1, anything else is malformed (Anchor rejects it too)
    fn flag(&mut self) -> Result<u8, ProgramError> {
        let v = self.u8()?;
        ensure!(v <= 1, BAD_ARGS);
        Ok(v)
    }
    /// borsh `Option<T>` tag (same 0/1 rule)
    fn some(&mut self) -> Result<bool, ProgramError> {
        Ok(self.flag()? == 1)
    }
}

/// Mutable fixed-size view of an account that is at least `N` bytes long (checked by the caller).
fn wv<const N: usize>(acc: &mut AccountView) -> &mut [u8; N] {
    // SAFETY: called after all reads/CPIs of the instruction; the caller checked `data_len() >= N`.
    unsafe { &mut *(acc.data_mut_ptr() as *mut [u8; N]) }
}
macro_rules! wr {
    ($b:expr, $o:expr, $v:expr) => {
        // SAFETY: constant offsets inside the fixed-size view.
        unsafe { core::ptr::write_unaligned($b.as_mut_ptr().add($o) as *mut _, $v) }
    };
}
fn put(b: &mut [u8], o: usize, k: &[u8; 32]) {
    // SAFETY: `o + 32 <= b.len()` for every call site (struct offsets, const-asserted).
    unsafe { core::ptr::copy_nonoverlapping(k.as_ptr(), b.as_mut_ptr().add(o), 32) }
}

pub fn dispatch(pid: &Address, accounts: &mut [AccountView], disc: u64, args: &[u8]) -> ProgramResult {
    let mut r = Rd { b: args };
    match disc {
        D_INIT_CONFIG => init_config(pid, accounts, &mut r),
        D_SET_CONFIG => set_config(pid, accounts, &mut r),
        D_UPSERT => upsert(pid, accounts, &mut r),
        D_BLOCK => block(pid, accounts),
        D_CREATE => create_order(pid, accounts, &mut r),
        D_CANCEL => cancel(pid, accounts, &mut r),
        D_KCANCEL => keeper_cancel(pid, accounts),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

/// Config account + `admin` signer check. Returns the config data.
#[cfg_attr(feature = "prof", inline(never))]
fn admin_cfg<'a>(pid: &Address, admin: &AccountView, cfg: &'a AccountView) -> Result<&'a [u8; C_LEN], ProgramError> {
    let c = owned::<C_LEN>(cfg, pid, DISC_CONFIG, BAD_ACCOUNTS)?;
    ensure!(admin.is_signer() && key!(c, C_ADMIN) == admin.address().as_array(), NOT_ADMIN);
    Ok(c)
}

// accounts: admin(w,s) config(w) delegate escrow program program_data system
#[cfg_attr(feature = "prof", inline(never))]
fn init_config(pid: &Address, accounts: &mut [AccountView], r: &mut Rd) -> ProgramResult {
    let (keeper, oracle, treasury) = (r.take::<32>()?, r.take::<32>()?, r.take::<32>()?);
    let max_age = r.u64()?;
    ensure!(max_age > 0 && max_age <= 300 && accounts.len() >= 7, INVALID_ARG);
    let (cb, db, eb);
    {
        let a = &*accounts;
        let admin = &a[0];
        ensure!(admin.is_signer() && *a[4].address() == *pid && *a[6].address() == SYSTEM, BAD_ACCOUNTS);
        // only the upgrade authority may initialise (nobody can front-run)
        let (pd, _) = Address::try_find_program_address(&[pid.as_array()], &LOADER_UPGRADEABLE).ok_or(BAD_ACCOUNTS)?;
        ensure!(pd == *a[5].address() && a[5].owned_by(&LOADER_UPGRADEABLE), BAD_ACCOUNTS);
        let p = view::<45>(&a[5])?;
        ensure!(rd!(p, 0, u32) == 3 && p[12] == 1 && key!(p, 13) == admin.address().as_array(), NOT_ADMIN);
        let (c, b1) = Address::try_find_program_address(&[b"config"], pid).ok_or(BAD_ACCOUNTS)?;
        let (dl, b2) = Address::try_find_program_address(&[b"delegate"], pid).ok_or(BAD_ACCOUNTS)?;
        let (es, b3) = Address::try_find_program_address(&[b"escrow"], pid).ok_or(BAD_ACCOUNTS)?;
        ensure!(c == *a[1].address() && dl == *a[2].address() && es == *a[3].address(), BAD_ACCOUNTS);
        (cb, db, eb) = (b1, b2, b3);
        let bump = [cb];
        let seeds = [Seed::from(b"config"), Seed::from(&bump)];
        make_account(admin, &a[1], rent(C_LEN), C_LEN as u64, pid, &[Signer::from(&seeds)])?;
    }
    let admin = *accounts[0].address();
    let c = wv::<C_LEN>(&mut accounts[1]);
    wr!(c, 0, DISC_CONFIG);
    put(c, C_ADMIN, admin.as_array());
    put(c, C_KEEPER, keeper);
    put(c, C_ORACLE, oracle);
    put(c, C_TREASURY, treasury);
    wr!(c, C_FEE, 0u16); // owner decision: the fee is switched ON only via set_config
    c[C_PAUSED] = 0;
    wr!(c, C_MAX_AGE, max_age);
    c[147] = cb;
    c[C_DELEGATE_BUMP] = db;
    c[149] = eb;
    Ok(())
}

// accounts: admin(s) config(w)
#[cfg_attr(feature = "prof", inline(never))]
fn set_config(pid: &Address, accounts: &mut [AccountView], r: &mut Rd) -> ProgramResult {
    ensure!(accounts.len() >= 2, BAD_ACCOUNTS);
    admin_cfg(pid, &accounts[0], &accounts[1])?;
    // SetConfigParams: admin, keeper, oracle, treasury: Option<Pubkey>; fee_bps: Option<u16>; paused: Option<bool>;
    // max_price_age_slots: Option<u64>
    let mut keys: [Option<&[u8; 32]>; 4] = [None; 4];
    for k in keys.iter_mut() {
        if r.some()? {
            *k = Some(r.take::<32>()?);
        }
    }
    let fee = if r.some()? { Some(r.u16()?) } else { None };
    let paused = if r.some()? { Some(r.flag()?) } else { None };
    let age = if r.some()? { Some(r.u64()?) } else { None };
    ensure!(fee.map_or(true, |f| f <= 300), FEE_TOO_HIGH);
    ensure!(age.map_or(true, |v| v > 0 && v <= 300), INVALID_ARG);
    let c = wv::<C_LEN>(&mut accounts[1]);
    for (k, off) in keys.iter().zip([C_ADMIN, C_KEEPER, C_ORACLE, C_TREASURY]) {
        if let Some(v) = k {
            put(c, off, v);
        }
    }
    if let Some(f) = fee {
        wr!(c, C_FEE, f);
    }
    if let Some(p) = paused {
        c[C_PAUSED] = p;
    }
    if let Some(v) = age {
        wr!(c, C_MAX_AGE, v);
    }
    Ok(())
}

// accounts: admin(w,s) config mint token_config(w) system [vault_base vault_quote]
#[cfg_attr(feature = "prof", inline(never))]
fn upsert(pid: &Address, accounts: &mut [AccountView], r: &mut Rd) -> ProgramResult {
    ensure!(accounts.len() >= 5, BAD_ACCOUNTS);
    let status = r.u8()?;
    let (base, quote) = (r.take::<32>()?, r.take::<32>()?);
    let tol = r.u16()?;
    let (tp, bump);
    {
        let a = &*accounts;
        admin_cfg(pid, &a[0], &a[1])?;
        let mint = &a[2];
        ensure!(status <= 2 && *mint.address() != NATIVE && *a[4].address() == SYSTEM, INVALID_ARG);
        tp = mint.owner();
        ensure!(tp == TOKEN || tp == TOKEN22, BAD_TOKEN_PROG);
        let m = view::<82>(mint)?;
        if status == 0 {
            ensure!(mint.data_len() == 82 && m[45] == 1, UNSUPPORTED_EXT);
        }
        let has = *base != [0u8; 32] || *quote != [0u8; 32];
        if has {
            ensure!(*base != [0u8; 32] && *quote != [0u8; 32] && tol > 0 && tol <= 10_000 && a.len() == 7, INVALID_ARG);
            ensure!(a[5].address().as_array() == base && a[6].address().as_array() == quote, BAD_ACCOUNTS);
            let (b, q) = (tok_any(&a[5])?, tok_any(&a[6])?);
            ensure!(key!(b, 0) == mint.address().as_array() && key!(q, 0) == NATIVE.as_array(), BAD_TOKEN_ACC);
        } else {
            ensure!(tol == 0, INVALID_ARG);
        }
        let (pda, b1) = Address::try_find_program_address(&[b"token", mint.address().as_array()], pid).ok_or(BAD_ACCOUNTS)?;
        ensure!(pda == *a[3].address(), BAD_ACCOUNTS);
        bump = b1;
        if a[3].owned_by(&SYSTEM) {
            let bs = [bump];
            let seeds = [Seed::from(b"token"), Seed::from(mint.address().as_array()), Seed::from(&bs)];
            make_account(&a[0], &a[3], rent(T_LEN), T_LEN as u64, pid, &[Signer::from(&seeds)])?;
        } else {
            ensure!(a[3].owned_by(pid), BAD_ACCOUNTS);
        }
    }
    let mint_key = *accounts[2].address();
    let t = wv::<T_LEN>(&mut accounts[3]);
    wr!(t, 0, DISC_TOKEN_CONFIG);
    put(t, T_MINT, mint_key.as_array());
    t[T_STATUS] = status;
    put(t, 41, tp.as_array());
    put(t, T_VAULT_BASE, base);
    put(t, T_VAULT_QUOTE, quote);
    wr!(t, T_TOLERANCE, tol);
    t[139] = bump;
    Ok(())
}

// accounts: admin(s) config token_config(w)
#[cfg_attr(feature = "prof", inline(never))]
fn block(pid: &Address, accounts: &mut [AccountView]) -> ProgramResult {
    ensure!(accounts.len() >= 3, BAD_ACCOUNTS);
    admin_cfg(pid, &accounts[0], &accounts[1])?;
    owned::<T_LEN>(&accounts[2], pid, DISC_TOKEN_CONFIG, TOKEN_NOT_ALLOWED)?;
    wv::<T_LEN>(&mut accounts[2])[T_STATUS] = 1;
    Ok(())
}

/// Source token account of an order: owner's, right mint, delegated to our PDA for >= `need`, balance >= `need`.
#[cfg_attr(feature = "prof", inline(never))]
fn check_source(src: &AccountView, owner: &[u8; 32], mint: &[u8; 32], tp: &Address, delegate: &Address, need: u64) -> ProgramResult {
    let s = tok(src, tp, BAD_TOKEN_ACC)?;
    ensure!(key!(s, 0) == mint && key!(s, 32) == owner, BAD_TOKEN_ACC);
    ensure!(rd!(s, 72, u32) == 1 && key!(s, 76) == delegate.as_array() && rd!(s, 121, u64) >= need, BAD_DELEGATE);
    ensure!(rd!(s, 64, u64) >= need, NO_BALANCE);
    Ok(())
}

#[cfg_attr(feature = "prof", inline(never))]
fn delegate_pda(pid: &Address, cfg: &[u8; C_LEN]) -> Result<Address, ProgramError> {
    let cb = [cfg[C_DELEGATE_BUMP]];
    Address::create_program_address(&[b"delegate", &cb], pid).map_err(|_| BAD_ACCOUNTS)
}

// accounts: owner(w,s) config order(w) give_mint source give_token_config delegate system [want token configs]
#[cfg_attr(feature = "prof", inline(never))]
fn create_order(pid: &Address, accounts: &mut [AccountView], r: &mut Rd) -> ProgramResult {
    let nonce = r.u64()?;
    let kind = r.u8()?;
    let total = r.u64()?;
    let wc = r.u32()? as usize;
    ensure!((1..=5).contains(&wc), INVALID_ARG);
    let mut wants = [[0u8; 32]; 5];
    for w in wants.iter_mut().take(wc) {
        *w = *r.take::<32>()?;
    }
    let (limit, abs, min_fill, partial, expires) = (r.u16()?, r.u64()?, r.u16()?, r.flag()?, r.u64()? as i64);
    let now = Clock::get()?.unix_timestamp;
    ensure!(accounts.len() == 8 + wc, BAD_ACCOUNTS);
    let bump;
    {
        let a = &*accounts;
        let (owner, give_mint, src) = (&a[0], &a[3], &a[4]);
        let cfg = owned::<C_LEN>(&a[1], pid, DISC_CONFIG, BAD_ACCOUNTS)?;
        ensure!(owner.is_signer() && *a[7].address() == SYSTEM, BAD_ACCOUNTS);
        ensure!(cfg[C_PAUSED] == 0, PAUSED);
        ensure!(total > 0, INVALID_ARG);
        ensure!(expires > now, EXPIRED); // r8 order of checks/errors
        ensure!(min_fill <= 10_000 && limit <= 10_000 && kind <= 2, INVALID_ARG);
        let gm = give_mint.address();
        let mut native_wanted = false;
        for (i, w) in wants.iter().enumerate().take(wc) {
            ensure!(w != gm.as_array() && !wants.iter().take(i).any(|x| x == w), INVALID_ARG);
            native_wanted |= w == NATIVE.as_array();
        }
        match kind {
            0 => ensure!(*gm != NATIVE && wc == 1 && native_wanted, INVALID_ARG),
            1 => ensure!(*gm == NATIVE && !native_wanted && abs > 0, INVALID_ARG),
            _ => {}
        }
        let tp = give_mint.owner();
        ensure!(tp == TOKEN || tp == TOKEN22, BAD_TOKEN_PROG);
        let tc_ok = |acc: &AccountView, mint: &[u8; 32]| -> ProgramResult {
            if mint == NATIVE.as_array() {
                return Ok(());
            }
            let tc = owned::<T_LEN>(acc, pid, DISC_TOKEN_CONFIG, TOKEN_NOT_ALLOWED)?;
            ensure!(key!(tc, T_MINT) == mint && tc[T_STATUS] == 0, TOKEN_NOT_ALLOWED);
            Ok(())
        };
        tc_ok(&a[5], gm.as_array())?;
        for (acc, w) in a.iter().skip(8).zip(wants.iter()).take(wc) {
            tc_ok(acc, w)?;
        }
        let dl = delegate_pda(pid, cfg)?;
        ensure!(dl == *a[6].address(), BAD_ACCOUNTS);
        check_source(src, owner.address().as_array(), gm.as_array(), &tp, &dl, total)?;
        let nb = nonce.to_le_bytes();
        let (okey, b1) = Address::try_find_program_address(&[b"order", owner.address().as_array(), &nb], pid).ok_or(BAD_ACCOUNTS)?;
        ensure!(okey == *a[2].address(), BAD_ACCOUNTS);
        bump = b1;
        let bs = [bump];
        let seeds = [Seed::from(b"order"), Seed::from(owner.address().as_array()), Seed::from(&nb), Seed::from(&bs)];
        make_account(owner, &a[2], rent(O_LEN), O_LEN as u64, pid, &[Signer::from(&seeds)])?;
    }
    let (owner_key, give_key) = (*accounts[0].address(), *accounts[3].address());
    let o = wv::<O_LEN>(&mut accounts[2]);
    wr!(o, 0, DISC_ORDER);
    put(o, O_OWNER, owner_key.as_array());
    o[O_KIND] = kind;
    put(o, O_GIVE_MINT, give_key.as_array());
    wr!(o, O_TOTAL, total);
    wr!(o, O_FILLED, 0u64);
    for (i, w) in wants.iter().enumerate() {
        put(o, O_WANTS + 32 * i, w);
    }
    o[O_WANT_COUNT] = wc as u8;
    wr!(o, O_LIMIT_BPS, limit);
    wr!(o, O_ABS_LIMIT, abs);
    wr!(o, O_MIN_FILL, min_fill);
    o[O_PARTIAL] = partial;
    wr!(o, O_EXPIRES, expires);
    o[O_STATUS] = 0;
    wr!(o, 272, nonce);
    wr!(o, 280, now);
    o[288] = bump;
    Ok(())
}

/// Return the order's rent to `rcv` and close it (zero data, system owner).
fn close_into(order: &mut AccountView, rcv: &mut AccountView) -> ProgramResult {
    let nl = rcv.lamports().checked_add(order.lamports()).ok_or(MATH)?;
    rcv.set_lamports(nl);
    order.close()
}

// accounts: owner(w,s) order(w) [source token program]   args: revoke bool
#[cfg_attr(feature = "prof", inline(never))]
fn cancel(pid: &Address, accounts: &mut [AccountView], r: &mut Rd) -> ProgramResult {
    ensure!(accounts.len() >= 2, BAD_ACCOUNTS);
    let revoke = r.flag()? != 0;
    {
        let a = &*accounts;
        let owner = &a[0];
        let o = owned::<O_LEN>(&a[1], pid, DISC_ORDER, BAD_ACCOUNTS)?;
        ensure!(owner.is_signer(), BAD_ACCOUNTS);
        ensure!(key!(o, O_OWNER) == owner.address().as_array(), NOT_OWNER);
        if revoke {
            ensure!(a.len() == 4, BAD_ACCOUNTS);
            let tpa = a[3].address();
            ensure!(*tpa == TOKEN || *tpa == TOKEN22, BAD_TOKEN_PROG);
            let s = tok(&a[2], tpa, BAD_TOKEN_ACC)?;
            ensure!(key!(s, 32) == owner.address().as_array() && key!(s, 0) == key!(o, O_GIVE_MINT), BAD_TOKEN_ACC);
            call(tpa, [&a[2], owner], [W, S], &[5], &[])?;
        }
    }
    let [owner, order, ..] = accounts else { return Err(BAD_ACCOUNTS) };
    close_into(order, owner)
}

// accounts: authority(s) config order(w) owner(w)
#[cfg_attr(feature = "prof", inline(never))]
fn keeper_cancel(pid: &Address, accounts: &mut [AccountView]) -> ProgramResult {
    ensure!(accounts.len() >= 4, BAD_ACCOUNTS);
    {
        let a = &*accounts;
        let auth = &a[0];
        let cfg = owned::<C_LEN>(&a[1], pid, DISC_CONFIG, BAD_ACCOUNTS)?;
        let o = owned::<O_LEN>(&a[2], pid, DISC_ORDER, BAD_ACCOUNTS)?;
        let k = auth.address().as_array();
        ensure!(auth.is_signer() && (key!(cfg, C_KEEPER) == k || key!(cfg, C_ADMIN) == k), NOT_KEEPER);
        ensure!(key!(o, O_OWNER) == a[3].address().as_array(), NOT_OWNER);
    }
    let [_, _, order, owner, ..] = accounts else { return Err(BAD_ACCOUNTS) };
    close_into(order, owner)
}
