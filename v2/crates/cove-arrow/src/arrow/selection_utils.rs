use super::*;

pub(crate) fn count_bitset_rows(words: &[u64], len: usize) -> Result<usize, CoveError> {
    let word_len = len.div_ceil(64);
    if words.len() < word_len {
        return Err(CoveError::BufferTooShort);
    }
    let mut count = 0usize;
    for (word_index, raw_word) in words.iter().take(word_len).copied().enumerate() {
        let word = if word_index + 1 == word_len {
            mask_selection_tail(raw_word, len)
        } else {
            raw_word
        };
        count = count
            .checked_add(word.count_ones() as usize)
            .ok_or(CoveError::ArithOverflow)?;
    }
    Ok(count)
}

pub(crate) fn mask_selection_tail(word: u64, len: usize) -> u64 {
    let tail_bits = len % 64;
    if tail_bits == 0 {
        word
    } else {
        word & ((1u64 << tail_bits) - 1)
    }
}

pub(crate) fn selected_rows_are_all_rows(selected_rows: &[u32], row_count: u64) -> bool {
    u64::try_from(selected_rows.len()).ok() == Some(row_count)
        && selected_rows
            .iter()
            .enumerate()
            .all(|(index, row)| u32::try_from(index).ok() == Some(*row))
}
