//! Admin and order-lifecycle instructions of the offchart-lite program: init_config, set_config, create_order, cancel_order, keeper_cancel_order
//! (update_order was cut: D-rM3-01; there are no TokenConfig instructions: any compatible mint trades without registration). Discriminators,
//! argument encodings and account orders follow the r8 layout; `init_config` / `set_config` / `create_order` validate the rCap limits record
//! (cap + format marker) in `Config.oracle`. Arithmetic table: qa/reports/rM3.md, DECISIONS D-rM3-* and D-rCap-*.
use super::*;

/// "OFFCAPv1" — format marker of the limits record in Config.oracle
pub(super) const LIMITS_MAGIC: [u8; 8] = *b"OFFCAPv1";
const D_INIT_CONFIG: u64 = u64::from_le_bytes([0x17, 0xeb, 0x73, 0xe8, 0xa8, 0x60, 0x01, 0xe7]);
const D_SET_CONFIG: u64 = u64::from_le_bytes([0x6c, 0x9e, 0x9a, 0xaf, 0xd4, 0x62, 0x34, 0x42]);
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

/// rCap: the Config `oracle` slot (32 B) is the LIMITS record, validated on every use (settle, create_order, init/set_config):
/// bytes 0..8 = max USD value of ONE order in 1e-6 USD (u64 LE, > 0); bytes 8..16 = the format marker `LIMITS_MAGIC`; bytes 16..32 = zero.
/// A pre-rCap Config (a real key in this slot) fails the marker check and is REFUSED (6043) — never reinterpreted as a cap.
pub(super) fn limits_ok(k: &[u8; 32]) -> bool {
    let mut z = 0u8;
    for b in k.iter().skip(16) {
        z |= *b;
    }
    z == 0 && k[8..16] == LIMITS_MAGIC && u64::from_le_bytes([k[0], k[1], k[2], k[3], k[4], k[5], k[6], k[7]]) > 0
}

// accounts: admin(w,s) config(w) delegate escrow program program_data system
#[cfg_attr(feature = "prof", inline(never))]
fn init_config(pid: &Address, accounts: &mut [AccountView], r: &mut Rd) -> ProgramResult {
    let (keeper, oracle, treasury) = (r.take::<32>()?, r.take::<32>()?, r.take::<32>()?);
    let max_age = r.u64()?;
    ensure!(max_age > 0 && max_age <= 300 && accounts.len() >= 7, INVALID_ARG);
    ensure!(limits_ok(oracle), INVALID_ARG);
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
        make_account(admin, &a[1], rent(C_LEN)?, C_LEN as u64, pid, &[Signer::from(&seeds)])?;
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
    ensure!(keys[2].map_or(true, |k| limits_ok(k)), INVALID_ARG);
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

// accounts: owner(w,s) config order(w) give_mint source delegate system [want mints, same order as args.wants]
// args: nonce, kind, total, wants: Vec<Pubkey>, min_outs: Vec<u64> (same length), min_fill_bps, partial, expires_at
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
    ensure!(r.u32()? as usize == wc, INVALID_ARG);
    let mut outs = [0u64; 5];
    for x in outs.iter_mut().take(wc) {
        *x = r.u64()?;
        ensure!(*x > 0, INVALID_ARG); // a floor of zero would mean "give it away"
    }
    let (min_fill, partial, expires) = (r.u16()?, r.flag()?, r.u64()? as i64);
    let now = Clock::get()?.unix_timestamp;
    ensure!(accounts.len() == 7 + wc, BAD_ACCOUNTS);
    let bump;
    {
        let a = &*accounts;
        let (owner, give_mint, src) = (&a[0], &a[3], &a[4]);
        let cfg = owned::<C_LEN>(&a[1], pid, DISC_CONFIG, BAD_ACCOUNTS)?;
        ensure!(owner.is_signer() && *a[6].address() == SYSTEM, BAD_ACCOUNTS);
        ensure!(cfg[C_PAUSED] == 0, PAUSED);
        ensure!(limits_ok(key!(cfg, C_ORACLE)), CONFIG_OLD); // rCap: no new order on a Config whose cap record is not in the current format
        ensure!(total > 0, INVALID_ARG);
        ensure!(expires > now, EXPIRED);
        ensure!(min_fill <= 10_000 && kind <= 2, INVALID_ARG);
        let gm = give_mint.address();
        let mut native_wanted = false;
        for (i, w) in wants.iter().enumerate().take(wc) {
            ensure!(w != gm.as_array() && !wants.iter().take(i).any(|x| x == w), INVALID_ARG);
            native_wanted |= w == NATIVE.as_array();
        }
        match kind {
            0 => ensure!(*gm != NATIVE && wc == 1 && native_wanted, INVALID_ARG),
            1 => ensure!(*gm == NATIVE && !native_wanted, INVALID_ARG),
            _ => {}
        }
        let tp = give_mint.owner();
        ensure!(tp == TOKEN || tp == TOKEN22, BAD_TOKEN_PROG);
        ensure!(*gm != NATIVE || tp == TOKEN, BAD_TOKEN_PROG);
        mint_ok(give_mint)?;
        // every wanted mint must be a real, supported mint at exactly the signed address
        for (acc, w) in a.iter().skip(7).zip(wants.iter()).take(wc) {
            ensure!(acc.address().as_array() == w, BAD_ACCOUNTS);
            ensure!(acc.owned_by(&TOKEN) || acc.owned_by(&TOKEN22), BAD_TOKEN_PROG);
            mint_ok(acc)?;
        }
        let dl = delegate_pda(pid, cfg)?;
        ensure!(dl == *a[5].address(), BAD_ACCOUNTS);
        check_source(src, owner.address().as_array(), gm.as_array(), &tp, &dl, total)?;
        let nb = nonce.to_le_bytes();
        let (okey, b1) = Address::try_find_program_address(&[b"order", owner.address().as_array(), &nb], pid).ok_or(BAD_ACCOUNTS)?;
        ensure!(okey == *a[2].address(), BAD_ACCOUNTS);
        bump = b1;
        let bs = [bump];
        let seeds = [Seed::from(b"order"), Seed::from(owner.address().as_array()), Seed::from(&nb), Seed::from(&bs)];
        make_account(owner, &a[2], rent(O_LEN)?, O_LEN as u64, pid, &[Signer::from(&seeds)])?;
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
        wr!(o, O_MINOUT + 8 * i, outs[i]);
    }
    o[O_WANT_COUNT] = wc as u8;
    wr!(o, 250, 0u16); // limit_bps: reserved (rAny: limits are absolute)
    wr!(o, 252, 0u64); // rCap: cumulative USD value of fills (1e-6 USD)
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
