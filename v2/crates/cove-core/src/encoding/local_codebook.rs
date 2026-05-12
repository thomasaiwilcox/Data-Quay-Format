//! Spec §20.3 — LocalCodebook encoding.
//!
//! A local codebook stores distinct page-local values once, then encodes a
//! stream of local indexes using one of the approved child cascades:
//!
//! * `LocalCodebook(BitPacked(local_index))`
//! * `LocalCodebook(Rle(local_index))`
//!
//! Codebook values are stored in their page physical representation; the
//! `Encoding` trait exposes the integer-compatible subset for the simplified
//! reference decoder used by conformance fixtures.

use crate::{constants::CoveEncodingKind, constants::CovePhysicalKind, CoveError};

use super::{
    bit_packed::{BitPacked, BitPackedPayload},
    rle::{Rle, RlePayload},
    Encoding,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalIndexEncoding {
    Rle,
    BitPacked,
}

impl LocalIndexEncoding {
    fn from_kind_raw(raw: u16) -> Option<Self> {
        match CoveEncodingKind::from_u16(raw)? {
            CoveEncodingKind::Rle => Some(Self::Rle),
            CoveEncodingKind::BitPacked => Some(Self::BitPacked),
            _ => None,
        }
    }

    fn encoding_kind(self) -> CoveEncodingKind {
        match self {
            Self::Rle => CoveEncodingKind::Rle,
            Self::BitPacked => CoveEncodingKind::BitPacked,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalIndexPayload {
    Rle(RlePayload),
    BitPacked(BitPackedPayload),
}

impl LocalIndexPayload {
    fn encoding(&self) -> LocalIndexEncoding {
        match self {
            Self::Rle(_) => LocalIndexEncoding::Rle,
            Self::BitPacked(_) => LocalIndexEncoding::BitPacked,
        }
    }

    fn encode(&self) -> Vec<u8> {
        match self {
            Self::Rle(payload) => payload.encode(),
            Self::BitPacked(payload) => payload.encode(),
        }
    }

    fn decode_local_indexes(&self) -> Result<Vec<u32>, CoveError> {
        let decoded = match self {
            Self::Rle(payload) => Rle::fast_decode(payload)?,
            Self::BitPacked(payload) => BitPacked::fast_decode(payload)?,
        };
        decoded
            .into_iter()
            .map(|value| u32::try_from(value).map_err(|_| CoveError::PageCorrupt))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalCodebookValueKind {
    FileCode,
    NumCode,
    Boolean,
    VarBytes,
}

impl LocalCodebookValueKind {
    fn from_raw(raw: u16) -> Result<Self, CoveError> {
        let physical_kind = u8::try_from(raw)
            .ok()
            .and_then(CovePhysicalKind::from_u8)
            .ok_or_else(|| {
                CoveError::UnsupportedEncoding(format!("LocalCodebook value physical kind {raw}"))
            })?;
        match physical_kind {
            CovePhysicalKind::FileCode => Ok(Self::FileCode),
            CovePhysicalKind::NumCode => Ok(Self::NumCode),
            CovePhysicalKind::Boolean => Ok(Self::Boolean),
            CovePhysicalKind::VarBytes => Ok(Self::VarBytes),
            _ => Err(CoveError::UnsupportedEncoding(format!(
                "LocalCodebook value physical kind {raw}"
            ))),
        }
    }

    fn to_raw(self) -> u16 {
        match self {
            Self::FileCode => CovePhysicalKind::FileCode as u16,
            Self::NumCode => CovePhysicalKind::NumCode as u16,
            Self::Boolean => CovePhysicalKind::Boolean as u16,
            Self::VarBytes => CovePhysicalKind::VarBytes as u16,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalCodebookValue {
    FileCode(u32),
    NumCode(u64),
    Boolean(bool),
    VarBytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalCodebookValues {
    FileCode(Vec<u32>),
    NumCode(Vec<u64>),
    Boolean(Vec<bool>),
    VarBytes(Vec<Vec<u8>>),
}

impl LocalCodebookValues {
    pub fn kind(&self) -> LocalCodebookValueKind {
        match self {
            Self::FileCode(_) => LocalCodebookValueKind::FileCode,
            Self::NumCode(_) => LocalCodebookValueKind::NumCode,
            Self::Boolean(_) => LocalCodebookValueKind::Boolean,
            Self::VarBytes(_) => LocalCodebookValueKind::VarBytes,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::FileCode(values) => values.len(),
            Self::NumCode(values) => values.len(),
            Self::Boolean(values) => values.len(),
            Self::VarBytes(values) => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn parse(
        kind: LocalCodebookValueKind,
        codebook_len: usize,
        bytes: &[u8],
    ) -> Result<Self, CoveError> {
        match kind {
            LocalCodebookValueKind::FileCode => {
                let expected = codebook_len
                    .checked_mul(4)
                    .ok_or(CoveError::ArithOverflow)?;
                if bytes.len() != expected {
                    return Err(CoveError::PageCorrupt);
                }
                let values = bytes
                    .chunks_exact(4)
                    .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
                    .collect();
                Ok(Self::FileCode(values))
            }
            LocalCodebookValueKind::NumCode => {
                let expected = codebook_len
                    .checked_mul(8)
                    .ok_or(CoveError::ArithOverflow)?;
                if bytes.len() != expected {
                    return Err(CoveError::PageCorrupt);
                }
                let values = bytes
                    .chunks_exact(8)
                    .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap()))
                    .collect();
                Ok(Self::NumCode(values))
            }
            LocalCodebookValueKind::Boolean => {
                if bytes.len() != codebook_len {
                    return Err(CoveError::PageCorrupt);
                }
                let mut values = Vec::with_capacity(codebook_len);
                for value in bytes {
                    match value {
                        0 => values.push(false),
                        1 => values.push(true),
                        _ => return Err(CoveError::PageCorrupt),
                    }
                }
                Ok(Self::Boolean(values))
            }
            LocalCodebookValueKind::VarBytes => {
                let mut values = Vec::with_capacity(codebook_len);
                let mut offset = 0usize;
                for _ in 0..codebook_len {
                    let length_end = offset.checked_add(4).ok_or(CoveError::ArithOverflow)?;
                    if length_end > bytes.len() {
                        return Err(CoveError::BufferTooShort);
                    }
                    let length =
                        u32::from_le_bytes(bytes[offset..length_end].try_into().unwrap()) as usize;
                    let value_end = length_end
                        .checked_add(length)
                        .ok_or(CoveError::ArithOverflow)?;
                    if value_end > bytes.len() {
                        return Err(CoveError::BufferTooShort);
                    }
                    values.push(bytes[length_end..value_end].to_vec());
                    offset = value_end;
                }
                if offset != bytes.len() {
                    return Err(CoveError::PageCorrupt);
                }
                Ok(Self::VarBytes(values))
            }
        }
    }

    fn encoded_len(&self) -> usize {
        match self {
            Self::FileCode(values) => values.len() * 4,
            Self::NumCode(values) => values.len() * 8,
            Self::Boolean(values) => values.len(),
            Self::VarBytes(values) => values.iter().map(|value| 4 + value.len()).sum(),
        }
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Self::FileCode(values) => {
                for value in values {
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
            Self::NumCode(values) => {
                for value in values {
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
            Self::Boolean(values) => {
                for value in values {
                    out.push(u8::from(*value));
                }
            }
            Self::VarBytes(values) => {
                for value in values {
                    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
                    out.extend_from_slice(value);
                }
            }
        }
    }

    fn value_at(&self, index: usize) -> Result<LocalCodebookValue, CoveError> {
        match self {
            Self::FileCode(values) => values
                .get(index)
                .copied()
                .map(LocalCodebookValue::FileCode)
                .ok_or(CoveError::PageCorrupt),
            Self::NumCode(values) => values
                .get(index)
                .copied()
                .map(LocalCodebookValue::NumCode)
                .ok_or(CoveError::PageCorrupt),
            Self::Boolean(values) => values
                .get(index)
                .copied()
                .map(LocalCodebookValue::Boolean)
                .ok_or(CoveError::PageCorrupt),
            Self::VarBytes(values) => values
                .get(index)
                .cloned()
                .map(LocalCodebookValue::VarBytes)
                .ok_or(CoveError::PageCorrupt),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCodebookPayload {
    pub values: LocalCodebookValues,
    pub indexes: LocalIndexPayload,
}

impl LocalCodebookPayload {
    /// Wire format (LE):
    /// `u16 child_kind | u16 value_physical_kind | u32 codebook_len | u32 child_len |
    ///  codebook_values | child_payload[child_len]`.
    pub fn parse(bytes: &[u8]) -> Result<Self, CoveError> {
        if bytes.len() < 12 {
            return Err(CoveError::BufferTooShort);
        }
        let child_raw = u16::from_le_bytes(bytes[0..2].try_into().unwrap());
        let value_kind =
            LocalCodebookValueKind::from_raw(u16::from_le_bytes(bytes[2..4].try_into().unwrap()))?;
        let codebook_len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let child_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let header_len = 12usize;
        if child_len > bytes.len().saturating_sub(header_len) {
            return Err(CoveError::BufferTooShort);
        }
        let child_start = bytes.len() - child_len;
        let values =
            LocalCodebookValues::parse(value_kind, codebook_len, &bytes[header_len..child_start])?;

        let child_bytes = &bytes[child_start..];
        let indexes = match LocalIndexEncoding::from_kind_raw(child_raw) {
            Some(LocalIndexEncoding::Rle) => {
                LocalIndexPayload::Rle(RlePayload::parse(child_bytes)?)
            }
            Some(LocalIndexEncoding::BitPacked) => {
                LocalIndexPayload::BitPacked(BitPackedPayload::parse(child_bytes)?)
            }
            None => {
                return Err(CoveError::UnsupportedEncoding(format!(
                    "LocalCodebook child encoding {child_raw}"
                )))
            }
        };

        let decoded = indexes.decode_local_indexes()?;
        if decoded
            .iter()
            .any(|index| (*index as usize) >= values.len())
        {
            return Err(CoveError::PageCorrupt);
        }

        Ok(Self { values, indexes })
    }

    pub fn encode(&self) -> Vec<u8> {
        let child_kind = self.indexes.encoding().encoding_kind() as u16;
        let child = self.indexes.encode();
        let mut out = Vec::with_capacity(12 + self.values.encoded_len() + child.len());
        out.extend_from_slice(&child_kind.to_le_bytes());
        out.extend_from_slice(&self.values.kind().to_raw().to_le_bytes());
        out.extend_from_slice(&(self.values.len() as u32).to_le_bytes());
        out.extend_from_slice(&(child.len() as u32).to_le_bytes());
        self.values.encode_into(&mut out);
        out.extend_from_slice(&child);
        out
    }

    pub fn decode_values(&self) -> Result<Vec<LocalCodebookValue>, CoveError> {
        let indexes = self.indexes.decode_local_indexes()?;
        indexes
            .into_iter()
            .map(|index| self.values.value_at(index as usize))
            .collect()
    }

    pub fn decode_file_codes(&self) -> Result<Vec<u32>, CoveError> {
        let LocalCodebookValues::FileCode(values) = &self.values else {
            return Err(CoveError::UnsupportedEncoding(
                "LocalCodebook is not FileCode".into(),
            ));
        };
        self.indexes
            .decode_local_indexes()?
            .into_iter()
            .map(|index| {
                values
                    .get(index as usize)
                    .copied()
                    .ok_or(CoveError::PageCorrupt)
            })
            .collect()
    }

    pub fn decode_num_codes(&self) -> Result<Vec<u64>, CoveError> {
        let LocalCodebookValues::NumCode(values) = &self.values else {
            return Err(CoveError::UnsupportedEncoding(
                "LocalCodebook is not NumCode".into(),
            ));
        };
        self.indexes
            .decode_local_indexes()?
            .into_iter()
            .map(|index| {
                values
                    .get(index as usize)
                    .copied()
                    .ok_or(CoveError::PageCorrupt)
            })
            .collect()
    }

    pub fn decode_booleans(&self) -> Result<Vec<bool>, CoveError> {
        let LocalCodebookValues::Boolean(values) = &self.values else {
            return Err(CoveError::UnsupportedEncoding(
                "LocalCodebook is not Boolean".into(),
            ));
        };
        self.indexes
            .decode_local_indexes()?
            .into_iter()
            .map(|index| {
                values
                    .get(index as usize)
                    .copied()
                    .ok_or(CoveError::PageCorrupt)
            })
            .collect()
    }

    pub fn decode_var_bytes(&self) -> Result<Vec<Vec<u8>>, CoveError> {
        let LocalCodebookValues::VarBytes(values) = &self.values else {
            return Err(CoveError::UnsupportedEncoding(
                "LocalCodebook is not VarBytes".into(),
            ));
        };
        self.indexes
            .decode_local_indexes()?
            .into_iter()
            .map(|index| {
                values
                    .get(index as usize)
                    .cloned()
                    .ok_or(CoveError::PageCorrupt)
            })
            .collect()
    }

    fn decode_i64_values(&self) -> Result<Vec<i64>, CoveError> {
        match &self.values {
            LocalCodebookValues::FileCode(_) => Ok(self
                .decode_file_codes()?
                .into_iter()
                .map(i64::from)
                .collect()),
            LocalCodebookValues::NumCode(_) => self
                .decode_num_codes()?
                .into_iter()
                .map(|value| i64::try_from(value).map_err(|_| CoveError::PageCorrupt))
                .collect(),
            LocalCodebookValues::Boolean(_) => {
                Ok(self.decode_booleans()?.into_iter().map(i64::from).collect())
            }
            LocalCodebookValues::VarBytes(_) => Err(CoveError::UnsupportedEncoding(
                "LocalCodebook VarBytes cannot decode through the i64 Encoding API".into(),
            )),
        }
    }
}

pub struct LocalCodebook;

impl Encoding for LocalCodebook {
    type Payload = LocalCodebookPayload;

    fn canonical_decode(payload: &Self::Payload) -> Result<Vec<i64>, CoveError> {
        payload.decode_i64_values()
    }

    fn fast_decode(payload: &Self::Payload) -> Result<Vec<i64>, CoveError> {
        payload.decode_i64_values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::assert_parity;

    #[test]
    fn round_trip_bit_packed_child() {
        let payload = LocalCodebookPayload {
            values: LocalCodebookValues::FileCode(vec![100, 200, 300]),
            indexes: LocalIndexPayload::BitPacked(
                BitPackedPayload::pack(&[0, 1, 2, 1, 0], 2).unwrap(),
            ),
        };
        let bytes = payload.encode();
        assert_eq!(LocalCodebookPayload::parse(&bytes).unwrap(), payload);
        assert_eq!(
            LocalCodebook::canonical_decode(&payload).unwrap(),
            vec![100, 200, 300, 200, 100]
        );
        assert!(assert_parity::<LocalCodebook>(&payload).is_ok());
    }

    #[test]
    fn round_trip_rle_child() {
        let payload = LocalCodebookPayload {
            values: LocalCodebookValues::NumCode(vec![7, 9]),
            indexes: LocalIndexPayload::Rle(RlePayload {
                runs: vec![(0, 3), (1, 1), (0, 2)],
            }),
        };
        let bytes = payload.encode();
        assert_eq!(LocalCodebookPayload::parse(&bytes).unwrap(), payload);
        assert_eq!(
            LocalCodebook::canonical_decode(&payload).unwrap(),
            vec![7, 7, 7, 9, 7, 7]
        );
    }

    #[test]
    fn rejects_out_of_range_local_index() {
        let payload = LocalCodebookPayload {
            values: LocalCodebookValues::FileCode(vec![42]),
            indexes: LocalIndexPayload::BitPacked(BitPackedPayload::pack(&[0, 1], 1).unwrap()),
        };
        assert_eq!(
            LocalCodebookPayload::parse(&payload.encode()),
            Err(CoveError::PageCorrupt)
        );
    }

    #[test]
    fn rejects_unsupported_child_kind() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(CoveEncodingKind::PlainFixed as u16).to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            LocalCodebookPayload::parse(&bytes),
            Err(CoveError::UnsupportedEncoding(_))
        ));
    }

    #[test]
    fn decodes_boolean_values() {
        let payload = LocalCodebookPayload {
            values: LocalCodebookValues::Boolean(vec![false, true]),
            indexes: LocalIndexPayload::BitPacked(
                BitPackedPayload::pack(&[0, 1, 1, 0], 1).unwrap(),
            ),
        };
        let parsed = LocalCodebookPayload::parse(&payload.encode()).unwrap();
        assert_eq!(
            parsed.decode_booleans().unwrap(),
            vec![false, true, true, false]
        );
        assert_eq!(
            LocalCodebook::canonical_decode(&parsed).unwrap(),
            vec![0, 1, 1, 0]
        );
    }

    #[test]
    fn decodes_var_bytes_values() {
        let payload = LocalCodebookPayload {
            values: LocalCodebookValues::VarBytes(vec![b"red".to_vec(), b"blue".to_vec()]),
            indexes: LocalIndexPayload::Rle(RlePayload {
                runs: vec![(1, 2), (0, 1)],
            }),
        };
        let parsed = LocalCodebookPayload::parse(&payload.encode()).unwrap();
        assert_eq!(
            parsed.decode_var_bytes().unwrap(),
            vec![b"blue".to_vec(), b"blue".to_vec(), b"red".to_vec()]
        );
        assert!(matches!(
            LocalCodebook::canonical_decode(&parsed),
            Err(CoveError::UnsupportedEncoding(_))
        ));
    }

    #[test]
    fn rejects_invalid_boolean_codebook_byte() {
        let mut bytes = LocalCodebookPayload {
            values: LocalCodebookValues::Boolean(vec![false]),
            indexes: LocalIndexPayload::Rle(RlePayload { runs: vec![(0, 1)] }),
        }
        .encode();
        bytes[12] = 2;
        assert_eq!(
            LocalCodebookPayload::parse(&bytes),
            Err(CoveError::PageCorrupt)
        );
    }

    #[test]
    fn rejects_mismatched_fixed_width_codebook_length_as_page_corrupt() {
        let mut bytes = LocalCodebookPayload {
            values: LocalCodebookValues::FileCode(vec![42]),
            indexes: LocalIndexPayload::Rle(RlePayload { runs: vec![(0, 1)] }),
        }
        .encode();
        bytes[4..8].copy_from_slice(&2u32.to_le_bytes());

        assert_eq!(
            LocalCodebookPayload::parse(&bytes),
            Err(CoveError::PageCorrupt)
        );
    }
}
