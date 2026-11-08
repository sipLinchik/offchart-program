//! Pure arithmetic of the fill / fee / floor checks and the exact ceiling used by the USD cap (`ceil_div_pow10`). No Solana types: fuzzed on the host.
//! (`seller_ok` / `buyer_ok` / `pool_ok` are legacy relative-limit helpers of the r8 model, kept for the host tests; `settle` does not call them.)
pub const BPS: u128 = 10_000;
pub const PRICE_SCALE: u128 = 1_000_000_000_000;

/// floor(amount * bps / 10_000) WITHOUT a 128-bit division (that alone costs ~5 KB of SBF code):
/// amount = q*10_000 + r  =>  q*bps + floor(r*bps/10_000), exact. Capped at `amount` (bps > 10_000 is not a valid fee).
pub fn fee_of(amount: u64, fee_bps: u16) -> u64 {
    let (q, r) = (amount / 10_000, amount % 10_000);
    let t = (q as u128) * (fee_bps as u128) + ((r * fee_bps as u64) / 10_000) as u128;
    if t > amount as u128 {
        amount
    } else {
        t as u64
    }
}

/// Seller / swapper side. `give` units of the give asset leave, `recv_net` units of the
/// received asset arrive (after fee). Prices are lamports*1e12 per raw unit.
/// Returns None on overflow, Some(ok) otherwise.
pub fn seller_ok(
    give: u64,
    give_price: u64,
    recv_net: u64,
    recv_price: u64,
    limit_bps: u16,
    abs_floor: u64,
) -> Option<bool> {
    if limit_bps as u128 > BPS {
        return None;
    }
    // a sale for nothing is never acceptable, whatever limit_bps the user signed
    if give == 0 || recv_net == 0 {
        return Some(false);
    }
    let value_give = (give as u128) * (give_price as u128);
    let value_recv = (recv_net as u128) * (recv_price as u128);
    let lhs = value_recv.checked_mul(BPS)?;
    let rhs = value_give.checked_mul(BPS - limit_bps as u128)?;
    if lhs < rhs {
        return Some(false);
    }
    if abs_floor > 0 && value_recv < (abs_floor as u128) * (give as u128) {
        return Some(false);
    }
    Some(true)
}

/// Buyer side: pays `give` (priced `give_price`), receives `recv_net` of the wanted asset.
/// `abs_cap` > 0 is the maximum price per raw unit of the wanted asset.
pub fn buyer_ok(
    give: u64,
    give_price: u64,
    recv_net: u64,
    recv_price: u64,
    limit_bps: u16,
    abs_cap: u64,
) -> Option<bool> {
    if abs_cap == 0 || recv_net == 0 {
        return Some(false);
    }
    let value_give = (give as u128) * (give_price as u128);
    let value_recv = (recv_net as u128) * (recv_price as u128);
    let lhs = value_give.checked_mul(BPS)?;
    let rhs = value_recv.checked_mul(BPS + limit_bps as u128)?;
    if lhs > rhs {
        return Some(false);
    }
    if value_give > (abs_cap as u128) * (recv_net as u128) {
        return Some(false);
    }
    Some(true)
}

/// |attested - pool| <= tolerance * pool, pool = quote*1e12/base.
///
/// Division-free form (r8 floors `pool = quote*1e12/base` first): with the exact rational pool price
/// P = quote*1e12/base the test |a - P|*1e4 <= P*tol is multiplied through by `base` — the verdict differs from r8
/// only when the price sits within one 1e-12 unit of the tolerance edge. Overflow is `None`, never a wrap.
pub fn pool_ok(attested: u64, quote_amount: u64, base_amount: u64, tolerance_bps: u16) -> Option<bool> {
    if base_amount == 0 || quote_amount == 0 {
        return Some(false);
    }
    let pool_x_base = (quote_amount as u128).checked_mul(PRICE_SCALE)?; // P * base
    let att_x_base = (attested as u128).checked_mul(base_amount as u128)?; // a * base
    let diff = if att_x_base > pool_x_base { att_x_base - pool_x_base } else { pool_x_base - att_x_base };
    Some(diff.checked_mul(BPS)? <= pool_x_base.checked_mul(tolerance_bps as u128)?)
}

/// rAny: the receiver\'s own floor. For its whole `total` it signed "at least `min_out`"; it gives `fill` and receives `net`.
/// net/fill >= min_out/total  <=>  net*total >= min_out*fill (both sides < 2^128, nothing can overflow).
pub fn floor_ok(net: u64, total: u64, min_out: u64, fill: u64) -> bool {
    min_out > 0 && (net as u128) * (total as u128) >= (min_out as u128) * (fill as u128)
}

/// Volume rules. `fill` > 0, <= remaining, >= min_fill unless it exhausts the order,
/// and a non-partial order must be filled whole in one go.
pub fn fill_ok(fill: u64, total: u64, filled: u64, min_fill_bps: u16, partial_ok: bool) -> bool {
    if fill == 0 || filled > total {
        return false;
    }
    let remaining = total - filled;
    if fill > remaining {
        return false;
    }
    if !partial_ok {
        return filled == 0 && fill == total;
    }
    if fill == remaining {
        return true;
    }
    let min = fee_of(total, min_fill_bps);
    fill >= min
}

/// ceil(num / 10^sh) for sh < 16 without a 128-bit division (see program.rs `usd_value`).
pub fn ceil_div_pow10(num: u128, sh: usize) -> u128 {
    let five = 5u64.pow((sh % 16) as u32);
    let s = (sh % 16) as u32;
    let lost = num & ((1u128 << s) - 1) != 0;
    let n1 = (num >> s) + lost as u128;
    let mut q: u128 = 0;
    let mut rem: u64 = 0;
    let mut i = 8u32;
    while i > 0 {
        i -= 1;
        let cur = (rem << 16) | ((n1 >> (16 * i)) & 0xffff) as u64;
        q = (q << 16) | (cur / five) as u128;
        rem = cur % five;
    }
    q + (rem != 0) as u128
}
