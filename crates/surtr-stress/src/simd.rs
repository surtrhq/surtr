//! The SIMD kernel: fused multiply-add on many independent lanes, the
//! densest power load a CPU core can run (the kind Linpack and Prime95's
//! small FFTs produce). Undervolt instability often shows only under it.
//!
//! One body is compiled for each instruction set (AVX-512, AVX2 with FMA,
//! NEON) and the best one the CPU has is picked at run time. `mul_add` is a
//! single IEEE rounding, so the result is bit-exact run after run, and the
//! known-answer check works as for the scalar kernels.

/// Independent lanes: enough vectors in flight to keep both FMA units busy
/// despite their latency.
const LANES: usize = 64;
/// Passes over the lanes per call (about 12.8 million FMAs).
const ROUNDS: usize = 200_000;

/// The work itself; inlined into each instruction-set variant so the
/// compiler vectorizes it with that set.
#[inline(always)]
fn body(seed: u64) -> u64 {
    let mut a = [0.0f64; LANES];
    let mut c = [0.0f64; LANES];
    let mut x = seed;
    for (a, c) in a.iter_mut().zip(c.iter_mut()) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        // Values in [0.5, 1.5) and small offsets: bounded, never denormal.
        *a = 0.5 + (x >> 11) as f64 / (1u64 << 53) as f64;
        *c = (x & 0xFFFF) as f64 / 1e9;
    }
    let m = 0.999_999_7_f64;
    for _ in 0..ROUNDS {
        for (a, c) in a.iter_mut().zip(c.iter()) {
            *a = a.mul_add(m, *c);
        }
    }
    a.iter().fold(0u64, |h, v| h.rotate_left(5) ^ v.to_bits())
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    #![allow(unsafe_code)]

    #[target_feature(enable = "avx512f,fma")]
    fn avx512(seed: u64) -> u64 {
        super::body(seed)
    }

    #[target_feature(enable = "avx2,fma")]
    fn avx2(seed: u64) -> u64 {
        super::body(seed)
    }

    pub fn fma_avx512(seed: u64) -> u64 {
        // SAFETY: only picked after `is_x86_feature_detected!` found
        // AVX-512F and FMA.
        unsafe { avx512(seed) }
    }

    pub fn fma_avx2(seed: u64) -> u64 {
        // SAFETY: only picked after `is_x86_feature_detected!` found AVX2
        // and FMA.
        unsafe { avx2(seed) }
    }
}

/// Without a SIMD set: still correct, much slower (and `mul_add` may be a
/// software fused multiply-add).
fn fma_scalar(seed: u64) -> u64 {
    body(seed)
}

/// The best SIMD variant this CPU runs: its name and function.
pub fn best() -> (&'static str, fn(u64) -> u64) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") && is_x86_feature_detected!("fma") {
            return ("fma-avx512", x86::fma_avx512);
        }
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            return ("fma-avx2", x86::fma_avx2);
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // NEON and FMA are part of every ARM64 CPU; the default build uses
        // them.
        return ("fma-neon", fma_scalar);
    }
    #[allow(unreachable_code)]
    ("fma-scalar", fma_scalar)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_agree_bit_for_bit() {
        let (name, best) = best();
        // Every variant computes the same fused operations in the same
        // order, so they must agree exactly with the plain build.
        for seed in [1, 0x243F_6A88_85A3_08D3] {
            assert_eq!(best(seed), fma_scalar(seed), "{name} differs");
            assert_eq!(best(seed), best(seed));
        }
        assert_ne!(best(1), best(2));
    }
}
