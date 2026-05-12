use super::*;

#[derive(Debug, Default)]
pub(crate) struct DecodeScratch {
    pub(crate) selected_mask: SelectionMask,
    pub(crate) filter_mask: SelectionMask,
    pub(crate) selected_rows: Vec<u32>,
    pub(crate) selection: Selection,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct SelectionMask {
    pub(crate) words: Vec<u64>,
    pub(crate) len: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) enum Selection {
    #[default]
    None,
    AllRows {
        len: usize,
    },
    Bitset(SelectionMask),
    RowIndices(Vec<u32>),
}

impl Selection {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::None => 0,
            Self::AllRows { len } => *len,
            Self::Bitset(mask) => mask.count_ones(),
            Self::RowIndices(rows) => rows.len(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn from_mask(mask: &SelectionMask, rows: &mut Vec<u32>) -> Result<Self, CoveError> {
        let selected = mask.count_ones();
        if selected == 0 {
            return Ok(Self::None);
        }
        if selected == mask.len {
            return Ok(Self::AllRows { len: mask.len });
        }
        if selected * 5 <= mask.len {
            mask.write_selected_rows(rows)?;
            return Ok(Self::RowIndices(rows.clone()));
        }
        Ok(Self::Bitset(mask.clone()))
    }

    pub(crate) fn from_rows(rows: &[u32], row_count: usize) -> Self {
        if rows.is_empty() {
            return Self::None;
        }
        if rows.len() == row_count
            && rows
                .iter()
                .enumerate()
                .all(|(index, row)| u32::try_from(index).ok() == Some(*row))
        {
            return Self::AllRows { len: row_count };
        }
        if rows.len() * 5 <= row_count {
            Self::RowIndices(rows.to_vec())
        } else {
            let mut mask = SelectionMask::default();
            mask.fill_none(row_count);
            for row in rows {
                let index = *row as usize;
                if index < row_count {
                    mask.set(index);
                }
            }
            Self::Bitset(mask)
        }
    }

    pub(crate) fn write_rows(&self, rows: &mut Vec<u32>) -> Result<(), CoveError> {
        rows.clear();
        match self {
            Self::None => Ok(()),
            Self::AllRows { len } => {
                rows.reserve(*len);
                for row in 0..*len {
                    rows.push(u32::try_from(row).map_err(|_| CoveError::ArithOverflow)?);
                }
                Ok(())
            }
            Self::Bitset(mask) => mask.write_selected_rows(rows),
            Self::RowIndices(values) => {
                rows.extend_from_slice(values);
                Ok(())
            }
        }
    }

    pub(crate) fn record(&self, stats: &mut DecodeStats) {
        match self {
            Self::None => stats.selection_none += 1,
            Self::AllRows { .. } => stats.selection_all_rows += 1,
            Self::Bitset(_) => stats.selection_bitsets += 1,
            Self::RowIndices(_) => stats.selection_row_indices += 1,
        }
    }
}

impl SelectionMask {
    pub(crate) fn clone_from_mask(&mut self, other: &Self) {
        self.len = other.len;
        self.words.clone_from(&other.words);
    }

    pub(crate) fn fill_all(&mut self, len: usize) {
        self.len = len;
        let word_len = len.div_ceil(64);
        self.words.clear();
        self.words.resize(word_len, u64::MAX);
        self.mask_tail();
    }

    pub(crate) fn fill_none(&mut self, len: usize) {
        self.len = len;
        let word_len = len.div_ceil(64);
        self.words.clear();
        self.words.resize(word_len, 0);
    }

    pub(crate) fn set(&mut self, index: usize) {
        debug_assert!(index < self.len);
        self.words[index / 64] |= 1u64 << (index % 64);
    }

    pub(crate) fn clear_bit(&mut self, index: usize) {
        debug_assert!(index < self.len);
        self.words[index / 64] &= !(1u64 << (index % 64));
    }

    pub(crate) fn and_inplace(&mut self, other: &Self) {
        debug_assert_eq!(self.len, other.len);
        for (left, right) in self.words.iter_mut().zip(other.words.iter()) {
            *left &= *right;
        }
    }

    pub(crate) fn all_zero(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }

    pub(crate) fn count_ones(&self) -> usize {
        self.words
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    pub(crate) fn write_selected_rows(&self, rows: &mut Vec<u32>) -> Result<(), CoveError> {
        rows.clear();
        rows.reserve(self.count_ones());
        for (word_index, word) in self.words.iter().copied().enumerate() {
            let mut remaining = word;
            while remaining != 0 {
                let bit = remaining.trailing_zeros() as usize;
                let index = word_index
                    .checked_mul(64)
                    .and_then(|base| base.checked_add(bit))
                    .ok_or(CoveError::ArithOverflow)?;
                if index < self.len {
                    rows.push(u32::try_from(index).map_err(|_| CoveError::ArithOverflow)?);
                }
                remaining &= remaining - 1;
            }
        }
        Ok(())
    }

    fn mask_tail(&mut self) {
        let tail_bits = self.len % 64;
        if tail_bits == 0 {
            return;
        }
        if let Some(last) = self.words.last_mut() {
            *last &= (1u64 << tail_bits) - 1;
        }
    }
}
