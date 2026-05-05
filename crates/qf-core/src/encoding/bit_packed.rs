//! Spec §20.3 — BitPacked encoding (unsigned).
//!
//! Packs `row_count` unsigned values of width `bits_per_value` into a
//! little-endian bit stream. `bits_per_value` MUST be in `1..=63`. Decoding
//! returns the values widened to `i64` (zero-extended).

use crate::QfError;

use super::Encoding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitPackedPayload {
    pub bits_per_value: u8,
    pub row_count: u32,
    pub bits: Vec<u8>,
}

impl BitPackedPayload {
    /// Wire format: `u8 bits_per_value | u32 row_count | u32 byte_len | bytes`.
    pub fn parse(bytes: &[u8]) -> Result<Self, QfError> {
        if bytes.len() < 9 {
            return Err(QfError::BufferTooShort);
        }
        let bpv = bytes[0];
        if !(1..=63).contains(&bpv) {
            return Err(QfError::PageCorrupt);
        }
        let row_count = u32::from_le_bytes(bytes[1..5].try_into().unwrap());
        let byte_len = u32::from_le_bytes(bytes[5..9].try_into().unwrap()) as usize;
        if bytes.len() < 9 + byte_len {
            return Err(QfError::BufferTooShort);
        }
        let need_bits = (row_count as u64) * (bpv as u64);
        let need_bytes = ((need_bits + 7) / 8) as usize;
        if byte_len < need_bytes {
            return Err(QfError::PageCorrupt);
        }
        Ok(Self {
            bits_per_value: bpv,
            row_count,
            bits: bytes[9..9 + byte_len].to_vec(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(9 + self.bits.len());
        out.push(self.bits_per_value);
        out.extend_from_slice(&self.row_count.to_le_bytes());
        out.extend_from_slice(&(self.bits.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.bits);
        out
    }

    /// Build a payload from a slice of unsigned values, packing
    /// `bits_per_value` bits each (little-endian within the bit stream).
    pub fn pack(values: &[u64], bits_per_value: u8) -> Result<Self, QfError> {
        if !(1..=63).contains(&bits_per_value) {
            return Err(QfError::PageCorrupt);
        }
        let mask: u64 = if bits_per_value == 64 {
            u64::MAX
        } else {
            (1u64 << bits_per_value) - 1
        };
        let total_bits = values.len() as u64 * bits_per_value as u64;
        let mut bits = vec![0u8; ((total_bits + 7) / 8) as usize];
        let mut bit_pos: u64 = 0;
        for v in values {
            if v & !mask != 0 {
                return Err(QfError::PageCorrupt);
            }
            for b in 0..bits_per_value {
                let one = (v >> b) & 1;
                if one == 1 {
                    let byte = (bit_pos / 8) as usize;
                    let off = (bit_pos % 8) as u8;
                    bits[byte] |= 1u8 << off;
                }
                bit_pos += 1;
            }
        }
        Ok(Self {
            bits_per_value,
            row_count: values.len() as u32,
            bits,
        })
    }
}

pub struct BitPacked;

impl Encoding for BitPacked {
    type Payload = BitPackedPayload;

    fn canonical_decode(payload: &Self::Payload) -> Result<Vec<i64>, QfError> {
        let bpv = payload.bits_per_value as usize;
        let mut out = Vec::with_capacity(payload.row_count as usize);
        for r in 0..payload.row_count as usize {
            let mut v: u64 = 0;
            for b in 0..bpv {
                let bit_pos = r * bpv + b;
                let byte = bit_pos / 8;
                let off = (bit_pos % 8) as u8;
                if byte >= payload.bits.len() {
                    return Err(QfError::PageCorrupt);
                }
                let one = ((payload.bits[byte] >> off) & 1) as u64;
                v |= one << b;
            }
            out.push(v as i64);
        }
        Ok(out)
    }

    fn fast_decode(payload: &Self::Payload) -> Result<Vec<i64>, QfError> {
        // Word-at-a-time fast path for bpv <= 32: read 8 bytes, mask out
        // groups of `bpv` bits using a sliding window.
        let bpv = payload.bits_per_value as usize;
        if bpv > 32 {
            return Self::canonical_decode(payload);
        }
        let mask: u64 = (1u64 << bpv) - 1;
        let mut out = Vec::with_capacity(payload.row_count as usize);
        let mut bit_pos: u64 = 0;
        for _ in 0..payload.row_count {
            let byte_off = (bit_pos / 8) as usize;
            let bit_off = (bit_pos % 8) as u8;
            // Load up to 8 bytes safely; pad with zeros at end of buffer.
            let mut buf = [0u8; 8];
            let avail = payload.bits.len().saturating_sub(byte_off).min(8);
            buf[..avail].copy_from_slice(&payload.bits[byte_off..byte_off + avail]);
            let word = u64::from_le_bytes(buf);
            let v = (word >> bit_off) & mask;
            out.push(v as i64);
            bit_pos += bpv as u64;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::assert_parity;

    #[test]
    fn round_trip_3_bit() {
        let vals: Vec<u64> = vec![0, 1, 2, 3, 4, 5, 6, 7, 0, 7, 4];
        let p = BitPackedPayload::pack(&vals, 3).unwrap();
        let bytes = p.encode();
        let parsed = BitPackedPayload::parse(&bytes).unwrap();
        assert_eq!(parsed, p);
        let decoded = BitPacked::canonical_decode(&p).unwrap();
        let expected: Vec<i64> = vals.iter().map(|v| *v as i64).collect();
        assert_eq!(decoded, expected);
        assert!(assert_parity::<BitPacked>(&p).is_ok());
    }

    #[test]
    fn parity_holds_for_many_widths() {
        for bpv in [1u8, 2, 4, 7, 12, 17, 24, 31] {
            let max = if bpv == 64 {
                u64::MAX
            } else {
                (1u64 << bpv) - 1
            };
            let vals: Vec<u64> = (0..50).map(|i| (i as u64 * 37) & max).collect();
            let p = BitPackedPayload::pack(&vals, bpv).unwrap();
            assert_parity::<BitPacked>(&p)
                .unwrap_or_else(|e| panic!("bpv={bpv} parity failed: {e}"));
        }
    }

    #[test]
    fn rejects_oversize_value() {
        // bpv=3 only allows 0..=7
        assert!(BitPackedPayload::pack(&[8], 3).is_err());
    }

    #[test]
    fn rejects_invalid_bits_per_value() {
        assert!(BitPackedPayload::pack(&[0], 0).is_err());
        assert!(BitPackedPayload::pack(&[0], 64).is_err());
    }
}
