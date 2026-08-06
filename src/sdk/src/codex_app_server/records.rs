//! Bounded line framing over the app-server child's stdout.
//!
//! The app-server protocol is one JSON object per line, so the obvious reader is
//! [`AsyncBufReadExt::lines`]. It is the wrong one here: it buffers a whole
//! record before returning it, so the cap that is supposed to protect the daemon
//! is only consulted once the memory has already been spent. A provider binary
//! that emits a gigabyte without a newline — broken, or an untrusted override —
//! would take the host down before the check ran.
//!
//! So framing is done by hand: bytes are accumulated from the buffered reader's
//! own window, and the moment the accumulation crosses [`MAX_LINE_BYTES`] the
//! buffer is released and the rest of the record is consumed and discarded as it
//! arrives. Memory stays bounded no matter what the child writes, and the
//! connection resynchronises on the next newline instead of dying.

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// A single record longer than this is discarded rather than buffered.
///
/// Matches the CLI seam's own record cap: a stdout line that never terminates is
/// a broken server, and holding an unbounded buffer for one would let it consume
/// the host.
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

/// What one read of the child's stdout produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Record {
    /// A complete record, within the cap, with any line terminator stripped.
    Line(String),
    /// A record that crossed [`MAX_LINE_BYTES`]. Its bytes were dropped as they
    /// arrived rather than buffered, so nothing of it survives to be parsed.
    Oversized,
    /// The child closed stdout.
    Eof,
}

/// Read one newline-delimited record, never holding more than
/// [`MAX_LINE_BYTES`] of it.
///
/// A record still open at EOF is returned as a [`Record::Line`] rather than
/// discarded — a final message without a trailing newline is well-formed enough
/// to act on, and the next call reports [`Record::Eof`].
///
/// # Errors
///
/// Propagates the underlying read error; the caller treats one as the transport
/// breaking.
pub async fn read_record<R>(reader: &mut R) -> std::io::Result<Record>
where
    R: AsyncBufRead + Unpin,
{
    let mut buffered: Vec<u8> = Vec::new();
    // Latched once the record is known to be too long. From then on the bytes
    // are consumed to find the newline, and nothing is retained.
    let mut oversized = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(finish(buffered, oversized, true));
        }
        match available.iter().position(|byte| *byte == b'\n') {
            Some(index) => {
                if !oversized {
                    buffered.extend_from_slice(&available[..index]);
                }
                reader.consume(index + 1);
                return Ok(finish(buffered, oversized, false));
            }
            None => {
                let consumed = available.len();
                if !oversized {
                    buffered.extend_from_slice(available);
                    if buffered.len() > MAX_LINE_BYTES {
                        oversized = true;
                        // Release it now. Waiting until the newline arrives is
                        // exactly the unbounded growth the cap exists to stop.
                        buffered = Vec::new();
                    }
                }
                reader.consume(consumed);
            }
        }
    }
}

/// Turn the accumulated bytes into the record they represent.
///
/// `at_eof` distinguishes the two ways a read ends: a newline always yields a
/// record, while EOF yields one only if something was pending.
fn finish(buffered: Vec<u8>, oversized: bool, at_eof: bool) -> Record {
    if oversized {
        return Record::Oversized;
    }
    if at_eof && buffered.is_empty() {
        return Record::Eof;
    }
    let mut line = String::from_utf8_lossy(&buffered).into_owned();
    // `lines()` strips `\r\n`; a server writing CRLF should frame identically
    // here.
    if line.ends_with('\r') {
        line.pop();
    }
    Record::Line(line)
}
