//! Shared list-selection and viewport arithmetic.
//!
//! TUI pages keep their domain state, but delegate cursor clamping and scroll
//! window calculations here so empty lists and shrinking collections behave
//! consistently.

/// Clamp `index` to an existing row, returning zero for an empty list.
pub(crate) fn clamp(index: usize, len: usize) -> usize {
    index.min(len.saturating_sub(1))
}

/// Move one row up or down within a list of `len` rows.
pub(crate) fn moved(index: usize, len: usize, up: bool) -> usize {
    let index = clamp(index, len);
    if up {
        index.saturating_sub(1)
    } else {
        (index + 1).min(len.saturating_sub(1))
    }
}

/// First model row for a viewport that keeps `selected` visible.
pub(crate) fn viewport_start(selected: usize, len: usize, visible: usize) -> usize {
    if len == 0 || visible == 0 {
        return 0;
    }
    clamp(selected, len)
        .saturating_sub(visible / 2)
        .min(len.saturating_sub(visible))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_shrinking_lists_are_safe() {
        assert_eq!(clamp(9, 0), 0);
        assert_eq!(moved(9, 0, false), 0);
        assert_eq!(clamp(9, 3), 2);
    }

    #[test]
    fn movement_and_viewports_stay_bounded() {
        assert_eq!(moved(0, 3, true), 0);
        assert_eq!(moved(0, 3, false), 1);
        assert_eq!(moved(2, 3, false), 2);
        assert_eq!(viewport_start(8, 10, 4), 6);
    }
}
