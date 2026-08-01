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

#[cfg(test)]
mod tests;

/// How many bytes fit without allocating.
///
/// Sized so `CellText` is exactly as wide as the `String` it replaced: 15 bytes
/// plus a length byte is 16, matching `Box<str>`'s pointer and length, and the
/// discriminant rides in the padding the enum already has.
const INLINE_CAP: usize = 15;

/// The inline-or-heap representation, kept private to this module.
///
/// `Inline`'s `len` is an invariant [`CellText::as_str`]'s unchecked slice
/// (`buf[..len]`) depends on, not just a stored value — a `len` past
/// [`INLINE_CAP`] slices out of bounds and panics. Rust enum-variant fields
/// always share the visibility of their enum (there is no way to mark just
/// `len` private), so the only way to keep this un-forgeable is to keep the
/// enum itself unreachable from outside the module: [`CellText`] wraps it in a
/// field private to *that* type instead, and every path in — [`CellText::new`],
/// [`CellText::blank`], and `From<&str>` — copies a real `&str` and computes
/// `len` itself.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Repr {
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

/// The text of one terminal cell.
///
/// See [`Repr`] for why the representation is a private field rather than a
/// public enum: an external caller cannot construct or match on an invalid
/// inline length.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CellText(Repr);

impl CellText {
    /// An empty cell.
    pub const fn new() -> Self {
        CellText(Repr::Inline {
            buf: [0; INLINE_CAP],
            len: 0,
        })
    }

    /// A single space — what a blank cell renders as.
    ///
    /// `const` so a screen's blank cells, which are most of them, cost a copy of
    /// a fixed value rather than a construction each.
    pub const fn blank() -> Self {
        let mut buf = [0; INLINE_CAP];
        buf[0] = b' ';
        CellText(Repr::Inline { buf, len: 1 })
    }

    /// The text as a string slice.
    pub fn as_str(&self) -> &str {
        match &self.0 {
            Repr::Inline { buf, len } => {
                // The only constructors copy a `&str` in, so these bytes are
                // always a valid prefix boundary of one. `unwrap_or` rather than
                // `expect` so a hypothetical bug degrades to a blank cell
                // instead of taking the render pass down with it.
                debug_assert!(std::str::from_utf8(&buf[..*len as usize]).is_ok());
                std::str::from_utf8(&buf[..*len as usize]).unwrap_or("")
            }
            Repr::Heap(text) => text,
        }
    }

    /// Whether the cell holds nothing.
    pub fn is_empty(&self) -> bool {
        match &self.0 {
            Repr::Inline { len, .. } => *len == 0,
            Repr::Heap(text) => text.is_empty(),
        }
    }

    /// Whether the text is stored inline (no heap allocation). Test-only: used
    /// to assert the allocation-avoiding path was actually taken.
    #[cfg(test)]
    fn is_inline(&self) -> bool {
        matches!(self.0, Repr::Inline { .. })
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
            return CellText(Repr::Heap(text.into()));
        }
        let mut buf = [0u8; INLINE_CAP];
        buf[..bytes.len()].copy_from_slice(bytes);
        CellText(Repr::Inline {
            buf,
            len: bytes.len() as u8,
        })
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
