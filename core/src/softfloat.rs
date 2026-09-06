//! Software IEEE-754 helpers for SoftNPU.
//!
//! Integer-only `binary32` add/mul and `binary16` ↔ `binary32` conversion.
//! This is a **reference model** for F16/F32 jobs, not a tensor ISA, not
//! libm, and not a hard-float HAL. Round-to-nearest-even. Canonical qNaN.
//! Subnormals flush to signed zero (FTZ).

/// Canonical quiet NaN (positive).
pub const F32_QNAN: u32 = 0x7fc0_0000;
pub const F32_POS_INF: u32 = 0x7f80_0000;
pub const F32_SIGN: u32 = 0x8000_0000;

const F32_EXP_MASK: u32 = 0x7f80_0000;
const F32_FRAC_MASK: u32 = 0x007f_ffff;
const F32_HIDDEN: u32 = 0x0080_0000;
const F32_BIAS: i32 = 127;

const F16_SIGN: u16 = 0x8000;
const F16_EXP_MASK: u16 = 0x7c00;
const F16_FRAC_MASK: u16 = 0x03ff;
const F16_QNAN: u16 = 0x7e00;
const F16_POS_INF: u16 = 0x7c00;

#[inline]
fn f32_sign(x: u32) -> u32 {
    x & F32_SIGN
}

#[inline]
fn f32_exp(x: u32) -> u32 {
    (x & F32_EXP_MASK) >> 23
}

#[inline]
fn f32_frac(x: u32) -> u32 {
    x & F32_FRAC_MASK
}

#[inline]
fn pack_f32(sign: u32, exp: u32, frac: u32) -> u32 {
    (sign & F32_SIGN) | ((exp & 0xff) << 23) | (frac & F32_FRAC_MASK)
}

fn is_nan32(x: u32) -> bool {
    f32_exp(x) == 0xff && f32_frac(x) != 0
}

fn is_inf32(x: u32) -> bool {
    f32_exp(x) == 0xff && f32_frac(x) == 0
}

fn is_zero32(x: u32) -> bool {
    (x & !F32_SIGN) == 0
}

/// Flush subnormals to signed zero. SoftNPU is FTZ; not a claim of
/// bit-exact IEEE including subnormals.
fn flush32(x: u32) -> u32 {
    if f32_exp(x) == 0 {
        f32_sign(x)
    } else {
        x
    }
}

fn canon_nan32(x: u32) -> u32 {
    F32_QNAN | f32_sign(x)
}

/// Normalize a (possibly subnormal) significand. Returns (unbiased exp, 24-bit sig).
fn unpack_norm(x: u32) -> (i32, u32) {
    let e = f32_exp(x);
    let f = f32_frac(x);
    if e == 0 {
        let mut sig = f;
        let mut shift = 0i32;
        while sig != 0 && (sig & F32_HIDDEN) == 0 {
            sig <<= 1;
            shift += 1;
        }
        (1 - F32_BIAS - shift, sig)
    } else {
        (e as i32 - F32_BIAS, f | F32_HIDDEN)
    }
}

/// Pack a 24-bit significand (hidden bit set for normals) with GRS rounding.
fn pack_round(sign: u32, exp: i32, sig: u64, extra: u64) -> u32 {
    // `sig` holds the 24-bit significand in the low 24 bits after normalize.
    // `extra` is leftover bits below the significand (sticky contributor).
    let mut exp = exp;
    let mut sig = sig;
    let mut extra = extra;

    if sig == 0 {
        return sign;
    }

    // Normalize so bit 23 is set (or we are in the subnormal path).
    while sig < (F32_HIDDEN as u64) && exp > (1 - F32_BIAS) {
        sig <<= 1;
        extra <<= 1;
        exp -= 1;
        if extra & (1u64 << 63) != 0 {
            // should not happen: extra is small
            extra = u64::MAX;
        }
    }
    while sig >= ((F32_HIDDEN as u64) << 1) {
        extra = (extra >> 1) | ((sig & 1) << 63);
        sig >>= 1;
        exp += 1;
    }

    if exp >= 255 - F32_BIAS + 1 {
        return sign | F32_POS_INF;
    }

    if exp <= -F32_BIAS {
        // Subnormal or underflow. Shift so the hidden bit falls off.
        let shift = (1 - F32_BIAS - exp) as u32;
        if shift >= 32 {
            return sign;
        }
        extra |= sig & ((1u64 << shift) - 1);
        // Keep a guard in extra's high bit after the shift.
        let guard = (sig >> (shift.saturating_sub(1))) & 1;
        sig >>= shift;
        let lsb = sig & 1;
        let sticky = extra != 0;
        if guard == 1 && (sticky || lsb == 1) {
            sig += 1;
        }
        return pack_f32(sign, 0, sig as u32);
    }

    // Round-to-nearest-even using leftover bits in `extra`.
    // After normalize, extra's MSB (bit 63) is the guard if we shifted;
    // when we did not, extra is the product/add residue below bit 0 of sig.
    let guard = (extra >> 63) & 1;
    let sticky = (extra << 1) != 0;
    if extra != 0 && guard == 0 && (extra >> 62) == 0 {
        // extra may be a small leftover without a dedicated guard bit.
        // Treat any non-zero extra as "greater than a tie" only if we
        // stored a true guard at bit 63. If extra is just sticky in the
        // low bits (no left-shift), round using half-ulp at bit 0.
    }
    let _ = (guard, sticky);

    // For the common path (mul/add feed extra as bits below the 24-bit sig
    // with guard at bit 63 after they left-justify), apply RNE.
    let lsb = sig & 1;
    let half = 1u64 << 63;
    if extra > half || (extra == half && lsb == 1) {
        sig += 1;
        if sig >= ((F32_HIDDEN as u64) << 1) {
            sig >>= 1;
            exp += 1;
            if exp >= 255 - F32_BIAS + 1 {
                return sign | F32_POS_INF;
            }
        }
    }

    pack_f32(sign, (exp + F32_BIAS) as u32, sig as u32)
}

/// IEEE-754 binary32 multiply (software).
pub fn mul_f32(a: u32, b: u32) -> u32 {
    let a = flush32(a);
    let b = flush32(b);
    let sign = f32_sign(a) ^ f32_sign(b);
    if is_nan32(a) {
        return canon_nan32(a);
    }
    if is_nan32(b) {
        return canon_nan32(b);
    }
    if is_inf32(a) {
        return if is_zero32(b) {
            F32_QNAN
        } else {
            sign | F32_POS_INF
        };
    }
    if is_inf32(b) {
        return if is_zero32(a) {
            F32_QNAN
        } else {
            sign | F32_POS_INF
        };
    }
    if is_zero32(a) || is_zero32(b) {
        return sign;
    }

    let (ea, sa) = unpack_norm(a);
    let (eb, sb) = unpack_norm(b);
    // 24×24 → 48-bit product. Binary point: [1,2) × [1,2) = [1,4).
    let prod = (sa as u64) * (sb as u64);
    let mut exp = ea + eb;
    if prod >= (1u64 << 47) {
        // [2,4): hidden bit at 47. 24-bit window is bits 47..24.
        exp += 1;
        let extra_low = prod & ((1u64 << 24) - 1);
        let sig = prod >> 24;
        let extra = extra_low << (64 - 24);
        return pack_round(sign, exp, sig, extra);
    }
    let extra_low = prod & ((1u64 << 23) - 1);
    let sig = prod >> 23;
    let extra = extra_low << (64 - 23);
    pack_round(sign, exp, sig, extra)
}

/// IEEE-754 binary32 add (software).
pub fn add_f32(a: u32, b: u32) -> u32 {
    let a = flush32(a);
    let b = flush32(b);
    if is_nan32(a) {
        return canon_nan32(a);
    }
    if is_nan32(b) {
        return canon_nan32(b);
    }
    if is_inf32(a) {
        if is_inf32(b) && f32_sign(a) != f32_sign(b) {
            return F32_QNAN;
        }
        return a;
    }
    if is_inf32(b) {
        return b;
    }
    if is_zero32(a) {
        return if is_zero32(b) {
            // (+0) + (−0) = +0; (−0) + (−0) = −0
            if f32_sign(a) != 0 && f32_sign(b) != 0 {
                F32_SIGN
            } else {
                0
            }
        } else {
            b
        };
    }
    if is_zero32(b) {
        return a;
    }

    // Operate on magnitudes; swap so |a| >= |b|.
    let (mut x, mut y) = if (a & !F32_SIGN) < (b & !F32_SIGN) {
        (b, a)
    } else {
        (a, b)
    };
    let sign = f32_sign(x);
    let sub = f32_sign(x) != f32_sign(y);
    x &= !F32_SIGN;
    y &= !F32_SIGN;

    let (ex, sx) = unpack_norm(x);
    let (ey, sy) = unpack_norm(y);
    let shift = (ex - ey) as u32;
    // Keep GRS in extra. Work with 24-bit sig in the high half of a 64-bit
    // register so we can shift y right and keep residue.
    let xs = (sx as u64) << 32;
    let mut ys = (sy as u64) << 32;
    if shift != 0 {
        if shift >= 64 {
            ys = 0;
        } else {
            let lost = ys & ((1u64 << shift) - 1);
            ys >>= shift;
            if lost != 0 {
                ys |= 1; // sticky in the lowest bit of the aligned field
            }
        }
    }

    if sub {
        let d = xs.wrapping_sub(ys);
        if d == 0 {
            return 0;
        }
        // Normalize: bring MSB of d to bit 32+23 = 55
        let mut d = d;
        let mut e = ex;
        while d < (1u64 << 55) {
            d <<= 1;
            e -= 1;
        }
        let sig = d >> 32;
        let extra = d << 32;
        return pack_round(sign, e, sig, extra);
    }

    let mut s = xs + ys;
    let mut e = ex;
    if s >= (1u64 << 56) {
        let extra_bit = s & 1;
        s >>= 1;
        e += 1;
        let sig = s >> 32;
        let extra = (s << 32) | extra_bit;
        return pack_round(sign, e, sig, extra);
    }
    let sig = s >> 32;
    let extra = s << 32;
    pack_round(sign, e, sig, extra)
}

/// IEEE-754 binary16 → binary32 (lossless for finite F16).
pub fn f16_to_f32(h: u16) -> u32 {
    let sign = ((h as u32) & (F16_SIGN as u32)) << 16;
    let exp = (h & F16_EXP_MASK) >> 10;
    let frac = h & F16_FRAC_MASK;
    if exp == 0 {
        if frac == 0 {
            return sign;
        }
        let mut f = frac as u32;
        let mut shift = 0u32;
        while f & 0x400 == 0 {
            f <<= 1;
            shift += 1;
        }
        f &= 0x3ff;
        let e = (1 - 15 + F32_BIAS - shift as i32) as u32;
        return pack_f32(sign, e, f << 13);
    }
    if exp == 31 {
        return if frac == 0 {
            sign | F32_POS_INF
        } else {
            F32_QNAN | sign
        };
    }
    let e = (exp as i32 - 15 + F32_BIAS) as u32;
    pack_f32(sign, e, (frac as u32) << 13)
}

/// IEEE-754 binary32 → binary16 (round-to-nearest-even).
pub fn f32_to_f16(s: u32) -> u16 {
    let sign = ((s >> 16) & (F16_SIGN as u32)) as u16;
    let exp = f32_exp(s);
    let frac = f32_frac(s);
    if exp == 0xff {
        return if frac != 0 {
            sign | F16_QNAN
        } else {
            sign | F16_POS_INF
        };
    }
    // f16_exp = (exp - 127) + 15 = exp - 112
    let e = exp as i32 - 112;
    if e >= 31 {
        return sign | F16_POS_INF;
    }
    if e <= 0 {
        // Underflow to F16 subnormal or zero.
        // Need (1−e) extra shifts of the 24-bit sig (hidden+frac).
        if exp == 0 && frac == 0 {
            return sign;
        }
        let sig = if exp == 0 {
            frac
        } else {
            frac | F32_HIDDEN
        };
        // Target: 10-bit F16 subnormal. F32 sig is 23/24 bits.
        // Value scale: shift right by (14 - unbiased_f32_exp) plus 13
        // to go from 23-bit frac to 10-bit.
        let unbiased = if exp == 0 {
            // subnormal f32: already tiny for f16
            return sign;
        } else {
            exp as i32 - F32_BIAS
        };
        // f16 subnormal: 2^{-14} * (mant/2^10)
        // f32: 2^{unbiased} * (1.frac)
        let shift = (14 + 13 - unbiased) as i32; // 23-bit → 10-bit + exp gap
        if shift >= 32 {
            return sign;
        }
        if shift <= 0 {
            // still a normal after all? treat as smallest normal path
            return sign | 0x0001;
        }
        let wide = (sig as u64) << 8;
        let lost = wide & ((1u64 << shift) - 1);
        let mut out = (wide >> shift) as u16;
        let half = 1u64 << (shift - 1);
        if lost > half || (lost == half && (out & 1) != 0) {
            out = out.saturating_add(1);
        }
        return sign | (out & F16_FRAC_MASK);
    }
    // Normal: take 10 of 23 frac bits, RNE on the rest.
    let mut f = frac >> 13;
    let rem = frac & 0x1fff;
    let half = 0x1000;
    let mut e = e as u32;
    if rem > half || (rem == half && (f & 1) != 0) {
        f += 1;
        if f == 0x400 {
            f = 0;
            e += 1;
            if e >= 31 {
                return sign | F16_POS_INF;
            }
        }
    }
    sign | ((e as u16) << 10) | (f as u16)
}

/// Multiply two F16 values via F32 software math, then round back.
pub fn mul_f16(a: u16, b: u16) -> u16 {
    f32_to_f16(mul_f32(f16_to_f32(a), f16_to_f32(b)))
}

/// Add two F16 values via F32 software math, then round back.
pub fn add_f16(a: u16, b: u16) -> u16 {
    f32_to_f16(add_f32(f16_to_f32(a), f16_to_f32(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(v: f32) -> u32 {
        v.to_bits()
    }

    fn close_bits(got: u32, expect: u32) {
        if is_nan32(expect) {
            assert!(is_nan32(got), "expected NaN, got {got:#x}");
            return;
        }
        assert_eq!(
            got, expect,
            "got {got:#010x} ({}) expect {expect:#010x} ({})",
            f32::from_bits(got),
            f32::from_bits(expect)
        );
    }

    #[test]
    fn mul_known() {
        close_bits(mul_f32(bits(2.0), bits(3.0)), bits(6.0));
        close_bits(mul_f32(bits(1.5), bits(2.0)), bits(3.0));
        close_bits(mul_f32(bits(-4.0), bits(0.5)), bits(-2.0));
        close_bits(mul_f32(bits(0.0), bits(7.0)), bits(0.0));
        close_bits(mul_f32(bits(-0.0), bits(7.0)), bits(-0.0));
        close_bits(mul_f32(F32_POS_INF, bits(2.0)), F32_POS_INF);
        assert!(is_nan32(mul_f32(F32_POS_INF, 0)));
    }

    #[test]
    fn add_known() {
        close_bits(add_f32(bits(1.0), bits(2.0)), bits(3.0));
        close_bits(add_f32(bits(-1.0), bits(1.0)), bits(0.0));
        close_bits(add_f32(bits(1.5), bits(1.5)), bits(3.0));
        close_bits(add_f32(0, bits(-0.0)), 0);
        close_bits(add_f32(F32_SIGN, F32_SIGN), F32_SIGN);
        close_bits(add_f32(F32_POS_INF, bits(1.0)), F32_POS_INF);
        assert!(is_nan32(add_f32(F32_POS_INF, F32_SIGN | F32_POS_INF)));
    }

    #[test]
    fn matches_host_grid() {
        let vals = [
            0.0f32, -0.0, 1.0, -1.0, 2.0, 0.5, 0.25, 3.0, 7.0, 19.0, 0.1, 1e-6, 1e6, -3.5, 1.5,
            43.0, 50.0, 0.75, 8.0,
        ];
        for &a in &vals {
            for &b in &vals {
                close_bits(mul_f32(bits(a), bits(b)), bits(a * b));
                close_bits(add_f32(bits(a), bits(b)), bits(a + b));
            }
        }
    }

    #[test]
    fn f16_roundtrip_normals() {
        // 1, 2, 3, 4, −2 are exact in F16.
        for v in [1.0f32, 2.0, 3.0, 4.0, -2.0, 0.5, 0.0, -0.0] {
            let h = f32_to_f16(bits(v));
            close_bits(f16_to_f32(h), bits(v));
        }
        assert_eq!(f16_to_f32(0x3c00), bits(1.0));
        assert_eq!(f16_to_f32(0x4000), bits(2.0));
        assert_eq!(f32_to_f16(bits(1.0)), 0x3c00);
        assert_eq!(mul_f16(0x4000, 0x4200), f32_to_f16(bits(6.0))); // 2*3
    }
}
