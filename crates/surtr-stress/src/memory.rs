//! Cache and memory kernels (plan, section 5.3; the small-FFT and
//! large-FFT idea, plus a memory pattern test).
//!
//! - An FFT small enough to stay in L2: the heat of a busy core with little
//!   memory traffic.
//! - An FFT far bigger than the caches: every pass streams through RAM, so
//!   it loads the memory controller, the caches and the fabric between them
//!   (Infinity Fabric on AMD); unstable memory or controller settings show
//!   up here and rarely anywhere else.
//! - A pattern test: fresh memory filled with an address-dependent pattern
//!   and its inverse, then read back, like a memory tester inside the OS.
//!
//! The FFT does the same operations in the same order every time, so its
//! result is bit-exact; any flipped bit along the way changes the checksum.

/// FFT size that fits in L2 (complex values; 128 KiB of data).
pub const CACHE_POINTS: usize = 1 << 13;
/// FFT size far beyond the caches (32 MiB of data).
pub const MEMORY_POINTS: usize = 1 << 21;
/// Pattern test size, in 64-bit words (32 MiB).
pub const PATTERN_WORDS: usize = 1 << 22;

fn mix(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// The discrete Fourier transform of `re` + i·`im` in place (radix-2,
/// length a power of two).
fn transform(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let (cos, sin): (Vec<f64>, Vec<f64>) = (0..n / 2)
        .map(|k| {
            let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
            (a.cos(), a.sin())
        })
        .unzip();
    let mut len = 2;
    while len <= n {
        let step = n / len;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = (cos[k * step], sin[k * step]);
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len *= 2;
    }
}

/// An FFT of `points` (a power of two) complex values made from `seed`;
/// returns a checksum of the result.
pub fn fft(points: usize, seed: u64) -> u64 {
    let mut re: Vec<f64> = (0..points)
        .map(|i| (mix(seed ^ i as u64) >> 11) as f64 / (1u64 << 53) as f64 - 0.5)
        .collect();
    let mut im = vec![0.0f64; points];
    transform(&mut re, &mut im);
    re.iter().zip(&im).fold(0u64, |h, (r, i)| {
        h.rotate_left(7) ^ r.to_bits() ^ i.to_bits().rotate_left(32)
    })
}

/// Fills fresh memory with a pattern from `seed`, checks it, does the same
/// with the inverse, and returns a checksum mixed with how many words read
/// back wrong (the same every time on healthy memory).
pub fn pattern(words: usize, seed: u64) -> u64 {
    let mut buf: Vec<u64> = (0..words as u64).map(|i| mix(seed ^ i)).collect();
    let mut wrong = 0u64;
    for (i, w) in buf.iter_mut().enumerate() {
        let expected = mix(seed ^ i as u64);
        wrong += u64::from(*w != expected);
        *w = !expected;
    }
    let mut sum = 0u64;
    for (i, w) in buf.iter().enumerate() {
        wrong += u64::from(*w != !mix(seed ^ i as u64));
        sum = sum.rotate_left(1) ^ *w;
    }
    sum ^ wrong.rotate_left(48)
}

/// The cache kernel.
pub fn fft_cache(seed: u64) -> u64 {
    fft(CACHE_POINTS, seed)
}

/// The memory FFT kernel.
pub fn fft_memory(seed: u64) -> u64 {
    fft(MEMORY_POINTS, seed)
}

/// The memory pattern kernel.
pub fn pattern_memory(seed: u64) -> u64 {
    pattern(PATTERN_WORDS, seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_of_an_impulse_is_flat() {
        let mut re = vec![0.0f64; 16];
        let mut im = vec![0.0f64; 16];
        re[0] = 1.0;
        transform(&mut re, &mut im);
        assert!(re.iter().all(|v| (v - 1.0).abs() < 1e-12), "{re:?}");
        assert!(im.iter().all(|v| v.abs() < 1e-12));
    }

    #[test]
    fn transform_matches_a_plain_dft() {
        let n = 32;
        let input: Vec<f64> = (0..n).map(|i| ((i * 7 % 11) as f64) - 5.0).collect();
        let (mut re, mut im) = (input.clone(), vec![0.0f64; n]);
        transform(&mut re, &mut im);
        for k in 0..n {
            let (mut sr, mut si) = (0.0, 0.0);
            for (t, x) in input.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (k * t) as f64 / n as f64;
                sr += x * a.cos();
                si += x * a.sin();
            }
            assert!(
                (re[k] - sr).abs() < 1e-9 && (im[k] - si).abs() < 1e-9,
                "bin {k}"
            );
        }
    }

    #[test]
    fn kernels_are_deterministic() {
        // Small sizes keep debug builds quick; the real sizes run the same
        // code.
        assert_eq!(fft(1 << 10, 1), fft(1 << 10, 1));
        assert_ne!(fft(1 << 10, 1), fft(1 << 10, 2));
        assert_eq!(pattern(1 << 12, 1), pattern(1 << 12, 1));
        assert_ne!(pattern(1 << 12, 1), pattern(1 << 12, 2));
    }
}
