//! Reed-Solomon error correction over GF(2^m) for the 2D codes that bring their own field (Han Xin: GF(256) with
//! x^8 + x^6 + x^5 + x + 1 and GF(16) for the function information; Grid Matrix: GF(128) with x^7 + x^3 + 1). The
//! generator polynomial has the roots a^1 .. a^n (a = 2), as in Zint's `reedsol.c` (BSD-3-Clause).

/// A Galois field GF(2^m) from its primitive polynomial (bit pattern, e.g. `0x163`).
pub struct Field {
    exp: Vec<u16>,
    log: Vec<u16>,
    /// 2^m - 1.
    order: usize,
}

impl Field {
    pub fn new(poly: u32) -> Field {
        let m = 31 - poly.leading_zeros();
        let order = (1usize << m) - 1;
        let (mut exp, mut log) = (vec![0u16; order * 2], vec![0u16; order + 1]);
        let mut x = 1u32;
        for i in 0..order {
            exp[i] = x as u16;
            exp[i + order] = x as u16;
            log[x as usize] = i as u16;
            x <<= 1;
            if x & (1 << m) != 0 {
                x ^= poly;
            }
        }
        Field { exp, log, order }
    }

    fn mul(&self, a: u16, b: u16) -> u16 {
        if a == 0 || b == 0 {
            return 0;
        }
        self.exp[(usize::from(self.log[usize::from(a)]) + usize::from(self.log[usize::from(b)])) % self.order]
    }

    /// The `n` check symbols of `data` (systematic, highest-degree first: they follow the data in the symbol).
    pub fn check(&self, data: &[u8], n: usize) -> Vec<u8> {
        // Generator g(x) = (x - a^1)(x - a^2)...(x - a^n), coefficients highest degree first.
        let mut g = vec![1u16];
        for i in 1..=n {
            let root = self.exp[i % self.order];
            let mut next = vec![0u16; g.len() + 1];
            for (j, c) in g.iter().enumerate() {
                next[j] ^= *c;
                next[j + 1] ^= self.mul(*c, root);
            }
            g = next;
        }
        let mut rem = vec![0u16; n];
        if n == 0 {
            return Vec::new();
        }
        for d in data {
            let factor = u16::from(*d) ^ rem[0];
            rem.rotate_left(1);
            rem[n - 1] = 0;
            for (r, c) in rem.iter_mut().zip(&g[1..]) {
                *r ^= self.mul(*c, factor);
            }
        }
        rem.into_iter().map(|v| v as u8).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_qr_field() {
        // GF(256) with x^8 + x^4 + x^3 + x^2 + 1 and roots a^0.. is QR's; with roots a^1.. it is Data Matrix's
        // (ISO/IEC 16022 Annex O example: "123456" as codewords 142 164 186 gives check words 114 25 5 88 102).
        let f = Field::new(0x12d);
        assert_eq!(f.check(&[142, 164, 186], 5), [114, 25, 5, 88, 102]);
        // n = 0 and an empty message are harmless.
        assert!(f.check(&[1, 2], 0).is_empty());
        assert_eq!(f.check(&[], 3), [0, 0, 0]);
    }
}
