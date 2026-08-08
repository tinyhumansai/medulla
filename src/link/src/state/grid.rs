//! Channel 1: a latest-wins row grid (protocol §4.4).
//!
//! ```text
//! diff:  rows    u32          the grid's row count after applying
//!        changed u32          how many rows follow
//!        repeat changed times:
//!          index u32
//!          len   u32
//!          body  len bytes
//! ```
//!
//! A terminal has the opposite requirement to a task queue: after a 40-second
//! outage the host should send the *current* screen, not 40 seconds of
//! scrollback. Because a diff is computed against whatever state the peer
//! actually holds, catching up costs one diff regardless of how long the link
//! was down — the property channel 1 exists for.
//!
//! This is a deliberately small stand-in for the SDK's screen model
//! (`protocol/screen/`: `build_frame`, `apply_frame`, `changed_rows`), which
//! supplies the real channel-1 diff once the bridge is wired up in Phase 2. It
//! is kept here so the transport can be built and its latest-wins behaviour
//! proved without the link crate depending on the SDK.

use super::types::{SspState, StateError};

/// Largest row count a decoded diff may declare.
///
/// The row count is a peer-supplied `u32` that [`RowGrid::apply_diff`] resizes
/// by, and the diff carrying it is only `MAX_DIFF_LEN` bytes — so an eight-byte
/// frame declaring `0xFFFF_FFFF` rows would otherwise ask for a ~100 GB
/// allocation and abort the process. No terminal is this tall; a diff above the
/// cap is refused as malformed, failing the frame instead of the process.
pub const MAX_GRID_ROWS: usize = u16::MAX as usize;

/// A grid of opaque rows, synchronised by sending only the rows that changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowGrid {
    rows: Vec<Vec<u8>>,
}

impl RowGrid {
    /// An empty grid.
    pub fn new() -> Self {
        RowGrid::default()
    }

    /// Replace the whole grid with `rows` — the "latest wins" update.
    pub fn set_rows(&mut self, rows: Vec<Vec<u8>>) {
        self.rows = rows;
    }

    /// Replace one row, growing the grid with empty rows if needed.
    pub fn set_row(&mut self, index: usize, row: Vec<u8>) {
        if self.rows.len() <= index {
            self.rows.resize(index + 1, Vec::new());
        }
        self.rows[index] = row;
    }

    /// The current rows.
    pub fn rows(&self) -> &[Vec<u8>] {
        &self.rows
    }
}

impl SspState for RowGrid {
    fn rebase_steps(&self) -> Vec<Self> {
        let mut steps = Vec::with_capacity(self.rows.len());
        let mut rows = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            rows.push(row.clone());
            steps.push(RowGrid { rows: rows.clone() });
        }
        steps
    }

    /// Only the rows that differ from `prev`, plus the new row count.
    ///
    /// A row that exists in `self` but not in `prev` counts as changed, so a
    /// grown grid is filled in; a shrunk grid needs no per-row bytes at all,
    /// only the count.
    fn diff_from(&self, prev: &Self) -> Vec<u8> {
        let mut changed: Vec<(u32, &Vec<u8>)> = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            if prev.rows.get(index) != Some(row) {
                changed.push((index as u32, row));
            }
        }
        let mut out =
            Vec::with_capacity(8 + changed.iter().map(|(_, r)| r.len() + 8).sum::<usize>());
        out.extend_from_slice(&(self.rows.len() as u32).to_be_bytes());
        out.extend_from_slice(&(changed.len() as u32).to_be_bytes());
        for (index, row) in changed {
            out.extend_from_slice(&index.to_be_bytes());
            out.extend_from_slice(&(row.len() as u32).to_be_bytes());
            out.extend_from_slice(row);
        }
        out
    }

    fn apply_diff(&mut self, diff: &[u8]) -> Result<(), StateError> {
        let (rows, updates) = decode_rows(diff)?;
        // Decoded *and validated* before anything is mutated: a refused diff
        // must leave the state untouched, which the transport relies on.
        if let Some((index, _)) = updates.iter().find(|(index, _)| *index >= rows) {
            return Err(StateError::Malformed(format!(
                "grid diff sets row {index} of a {rows}-row grid"
            )));
        }
        self.rows.resize(rows, Vec::new());
        for (index, row) in updates {
            self.rows[index] = row;
        }
        Ok(())
    }
}

/// A decoded grid diff: the row count the sender ended on, and the rows it
/// changed, each as its index and its new contents.
type DecodedRows = (usize, Vec<(usize, Vec<u8>)>);

/// Parse a grid diff into its row count and its (index, row) updates.
fn decode_rows(diff: &[u8]) -> Result<DecodedRows, StateError> {
    let read_u32 = |at: usize| -> Result<usize, StateError> {
        if diff.len() < at + 4 {
            return Err(StateError::Malformed(format!(
                "grid diff truncated at offset {at}"
            )));
        }
        let mut raw = [0u8; 4];
        raw.copy_from_slice(&diff[at..at + 4]);
        Ok(u32::from_be_bytes(raw) as usize)
    };

    let rows = read_u32(0)?;
    // Bounded before the count reaches `resize`: the sender is untrusted and the
    // count is a bare u32 that costs 24 bytes of allocation per row.
    if rows > MAX_GRID_ROWS {
        return Err(StateError::Malformed(format!(
            "grid diff declares {rows} rows, above the {MAX_GRID_ROWS}-row maximum"
        )));
    }
    let changed = read_u32(4)?;
    let mut offset = 8;
    let mut updates = Vec::with_capacity(changed.min(1024));
    for _ in 0..changed {
        let index = read_u32(offset)?;
        let len = read_u32(offset + 4)?;
        offset += 8;
        if diff.len() < offset + len {
            return Err(StateError::Malformed(format!(
                "grid diff declares a {len}-byte row with only {} bytes left",
                diff.len() - offset
            )));
        }
        updates.push((index, diff[offset..offset + len].to_vec()));
        offset += len;
    }
    if offset != diff.len() {
        return Err(StateError::Malformed(format!(
            "grid diff has {} trailing bytes",
            diff.len() - offset
        )));
    }
    Ok((rows, updates))
}
