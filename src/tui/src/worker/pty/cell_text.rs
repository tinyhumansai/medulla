//! [`CellText`] — the contents of one terminal cell, stored inline.
//!
//! A screen snapshot is `rows × cols` cells, and every one of them used to hold
//! a `String`. At the default 120×30 that is 3,600 heap allocations per
//! snapshot, and snapshots are taken by the render loop every frame, by every
//! screen subscriber ten times a second, and — during a fan-out, the case that
//! actually hurts — by the prompt injector at 40 Hz for every session that is
//! still starting up. Ten harnesses coming up at once was over a million
//! allocations a second, all of it for text that is almost always a single
//! ASCII byte.
//!
//! A terminal cell holds one grapheme cluster. One is normally 1 byte, is 4 for
//! the widest single scalar, and only reaches double figures for emoji built out
//! of zero-width joiners. [`INLINE_CAP`] covers everything short of those, and
//! the rest fall back to the heap, so the representation is never wrong — only
//! sometimes slower, for cells that are vanishingly rare.
//!
//! Same size as the `String` it replaces (24 bytes), so nothing downstream grew.

/// How many bytes fit without allocating.
///
/// Sized so `CellText` is exactly as wide as the `String` it replaced: 15 bytes
/// plus a length byte is 16, matching `Box<str>`'s pointer and length, and the
/// discriminant rides in the padding the enum already has.
const INLINE_CAP: usize = 15;

/// The text of one terminal cell.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum CellText {
    /// Short enough to live in the struct — the overwhelmingly common case.
    Inline {
        /// The UTF-8 bytes; only `len` of them are meaningful.
        buf: [u8; INLINE_CAP],
        /// How many of `buf`'s bytes are in use.
        len: u8,
    },
    /// A grapheme cluster longer than [`INLINE_CAP`] (joined emoji, mostly).
    Heap(Box<str>),
}

impl CellText {
    /// An empty cell.
    pub const fn new() -> Self {
        CellText::Inline {
            buf: [0; INLINE_CAP],
            len: 0,
        }
    }

    /// A single space — what a blank cell renders as.
    ///
    /// `const` so a screen's blank cells, which are most of them, cost a copy of
    /// a fixed value rather than a construction each.
    pub const fn blank() -> Self {
        let mut buf = [0; INLINE_CAP];
        buf[0] = b' ';
        CellText::Inline { buf, len: 1 }
    }

    /// The text as a string slice.
    pub fn as_str(&self) -> &str {
        match self {
            CellText::Inline { buf, len } => {
                // The only constructor copies a `&str` in, so these bytes are
                // always a valid prefix boundary of one. `unwrap_or` rather than
                // `expect` so a hypothetical bug degrades to a blank cell
                // instead of taking the render pass down with it.
                debug_assert!(std::str::from_utf8(&buf[..*len as usize]).is_ok());
                std::str::from_utf8(&buf[..*len as usize]).unwrap_or("")
            }
            CellText::Heap(text) => text,
        }
    }

    /// Whether the cell holds nothing.
    pub fn is_empty(&self) -> bool {
        match self {
            CellText::Inline { len, .. } => *len == 0,
            CellText::Heap(text) => text.is_empty(),
        }
    }
}

impl Default for CellText {
    fn default() -> Self {
        CellText::new()
    }
}

impl From<&str> for CellText {
    fn from(text: &str) -> Self {
        let bytes = text.as_bytes();
        if bytes.len() > INLINE_CAP {
            return CellText::Heap(text.into());
        }
        let mut buf = [0u8; INLINE_CAP];
        buf[..bytes.len()].copy_from_slice(bytes);
        CellText::Inline {
            buf,
            len: bytes.len() as u8,
        }
    }
}

impl From<String> for CellText {
    fn from(text: String) -> Self {
        CellText::from(text.as_str())
    }
}

impl std::ops::Deref for CellText {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for CellText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Prints as the string it holds, so snapshot assertions read the same as they
/// did when this was a `String`.
impl std::fmt::Debug for CellText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_str(), f)
    }
}

impl PartialEq<str> for CellText {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for CellText {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_stays_inline() {
        for text in ["", " ", "a", "é", "▀", "😀"] {
            let cell = CellText::from(text);
            assert!(
                matches!(cell, CellText::Inline { .. }),
                "{text:?} should not allocate"
            );
            assert_eq!(cell.as_str(), text);
        }
    }

    #[test]
    fn a_long_grapheme_cluster_falls_back_to_the_heap() {
        // A ZWJ family emoji is 25 bytes — past the inline capacity, and the
        // reason the fallback exists rather than truncating.
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
        assert!(family.len() > INLINE_CAP);
        let cell = CellText::from(family);
        assert!(matches!(cell, CellText::Heap(_)));
        assert_eq!(cell.as_str(), family);
    }

    #[test]
    fn the_boundary_length_is_still_inline() {
        let text = "x".repeat(INLINE_CAP);
        assert!(matches!(
            CellText::from(text.as_str()),
            CellText::Inline { .. }
        ));
        assert_eq!(CellText::from(text.as_str()).as_str(), text);

        let over = "x".repeat(INLINE_CAP + 1);
        assert!(matches!(CellText::from(over.as_str()), CellText::Heap(_)));
    }

    #[test]
    fn default_is_empty() {
        assert!(CellText::default().is_empty());
        assert_eq!(CellText::default().as_str(), "");
    }

    #[test]
    fn it_is_no_wider_than_the_string_it_replaced() {
        // The point of the inline representation is to remove an allocation, not
        // to trade it for a fatter snapshot.
        assert!(std::mem::size_of::<CellText>() <= std::mem::size_of::<String>());
    }

    #[test]
    fn equality_matches_the_underlying_text() {
        assert_eq!(CellText::from("ab"), CellText::from("ab"));
        assert_ne!(CellText::from("ab"), CellText::from("ac"));
        // Bound first: comparing a temporary trips `clippy::cmp_owned`, which
        // does not know that a short `CellText` never allocates.
        let cell = CellText::from("ab");
        assert!(cell == *"ab");
        assert!(cell == "ab");
    }
}
