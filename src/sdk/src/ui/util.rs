//! Small display helpers shared by the views.

/// 24h clock `HH:MM:SS` for an epoch-ms timestamp (UTC — no tz database here).
pub fn clock(millis: i64) -> String {
    let secs = millis.div_euclid(1000).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Collapse internal whitespace, trim, and ellipsize to `width` chars.
pub fn clip(value: &str, width: usize) -> String {
    let single = collapse_ws(value);
    let len = single.chars().count();
    if len > width {
        let take = width.saturating_sub(1);
        let mut out: String = single.chars().take(take).collect();
        out.push('…');
        out
    } else {
        single
    }
}

/// Like [`clip`], but drops characters from the *front*.
///
/// For filesystem paths, where the tail identifies the thing and the head is
/// usually a home directory every candidate shares: clipping a path from the
/// right leaves several rows reading `/Users/someone/work/…`, which distinguishes
/// nothing.
pub fn clip_left(value: &str, width: usize) -> String {
    let single = collapse_ws(value);
    let len = single.chars().count();
    if len <= width {
        return single;
    }
    // One char is spent on the leading ellipsis, so keep `width - 1` from the
    // tail. A width of 0 or 1 leaves nothing meaningful; return just the marker
    // rather than panicking on the arithmetic.
    let keep = width.saturating_sub(1);
    let skip = len.saturating_sub(keep);
    let mut out = String::from("…");
    out.extend(single.chars().skip(skip));
    out
}

/// Ellipsize the middle while preserving both identifying ends.
///
/// This is the path-shaped counterpart to [`clip`]: a deep workspace usually
/// shares its leading directories with its neighbours, while its final
/// component names the project. Keeping both sides makes the shortened value
/// recognisable without letting it widen the pane that contains it.
pub fn clip_middle(value: &str, width: usize) -> String {
    let single = collapse_ws(value);
    let chars: Vec<char> = single.chars().collect();
    if chars.len() <= width {
        return single;
    }
    if width <= 1 {
        return "…".to_string();
    }
    let available = width - 1;
    let head_len = available.div_ceil(2);
    let tail_len = available / 2;
    let head: String = chars.iter().take(head_len).collect();
    let tail: String = chars.iter().skip(chars.len() - tail_len).collect();
    format!("{head}…{tail}")
}

fn collapse_ws(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How many characters of an address survive at each end.
const ADDRESS_EDGE: usize = 4;

/// Shorten a machine address for display: `abcd…1234`.
///
/// A tiny.place address is a Solana public key — 32 to 44 base58 characters of
/// pure entropy. Rendered whole it is the widest thing in the Agents rail, and
/// it sizes the whole sidebar, because the rail is measured against its widest
/// row. It also distinguishes nothing a human reads: the middle is noise, and
/// two peers are told apart by their ends.
///
/// Clipping from the right ([`clip`]) is the wrong shape here — several peers'
/// keys can share a prefix, so `AbCdEf…` rows would all look alike. Keeping both
/// ends is what keeps them distinguishable.
///
/// Unconditional: it shortens anything longer than the shortened form, so it is
/// for slots that are *known* to hold an opaque id. A slot that might hold a
/// name the operator chose wants [`short_if_address`] instead — this function
/// would happily turn `this-device` into `this…vice`.
pub fn short_address(value: &str) -> String {
    let trimmed = value.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    // The ellipsis has to save at least one character to be worth printing.
    if chars.len() <= ADDRESS_EDGE * 2 + 1 {
        return trimmed.to_string();
    }
    let head: String = chars.iter().take(ADDRESS_EDGE).collect();
    let tail: String = chars.iter().skip(chars.len() - ADDRESS_EDGE).collect();
    format!("{head}…{tail}")
}

/// Whether `value` looks like a bare machine address rather than a name someone
/// chose.
///
/// Shortening is only ever right for the former: a label, a handle, or a
/// workspace path is meaningful text, and cutting its middle out destroys the
/// thing that made it readable. A base58 public key has no spaces, no
/// punctuation a human would type, and a length in the key range.
pub fn looks_like_address(value: &str) -> bool {
    let trimmed = value.trim();
    (32..=44).contains(&trimmed.chars().count())
        && trimmed.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Shorten `value` only if it looks like a bare address, otherwise return it
/// unchanged.
///
/// The safe default for a display slot that *might* hold an address and might
/// hold a name the operator gave the machine.
pub fn short_if_address(value: &str) -> String {
    if looks_like_address(value) {
        short_address(value)
    } else {
        value.trim().to_string()
    }
}

/// Compact token count, e.g. `980` · `1.2k` · `34k`.
pub fn fmt_tokens(n: i64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    // Millions get their own unit, or a 1M context window renders as `1000k` —
    // technically correct, and the reason the rail read `ctx 1.2k/1000k` where
    // it should say `1M`. A round million drops the `.0`, matching
    // [`crate::ui::meters::tokens`] so the rail and the meter agree.
    if n >= 1_000_000 {
        let m = n as f64 / 1_000_000.0;
        return if (m - m.round()).abs() < f64::EPSILON {
            format!("{}M", m.round() as i64)
        } else {
            format!("{m:.1}M")
        };
    }
    let k = n as f64 / 1_000.0;
    if k >= 10.0 {
        format!("{}k", k.round() as i64)
    } else {
        format!("{:.1}k", k)
    }
}

/// Word-wrap `text` to `width` columns, expanding tabs to two spaces and
/// preserving hard newlines. Long unbreakable runs are hard-cut at `width`.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let line = raw.replace('\t', "  ");
        let line = line.trim_end();
        if line.chars().count() <= width {
            out.push(line.to_string());
            continue;
        }
        let mut rest: Vec<char> = line.chars().collect();
        while rest.len() > width {
            // Last space at or before `width`.
            let cut = rest[..=width.min(rest.len() - 1)]
                .iter()
                .rposition(|&c| c == ' ')
                .filter(|&i| i > 0)
                .unwrap_or(width);
            let head: String = rest[..cut].iter().collect();
            out.push(head);
            let mut tail: Vec<char> = rest[cut..].to_vec();
            while tail.first() == Some(&' ') {
                tail.remove(0);
            }
            rest = tail;
        }
        if !rest.is_empty() {
            out.push(rest.into_iter().collect());
        }
    }
    out
}

/// Words a session slug carries at most.
///
/// A session name is scanned in a rail or a list, never read: the fourth word
/// is always the one that pushes the identifying words off the end of a narrow
/// row, so the shape stops at three.
pub const SLUG_MAX_WORDS: usize = 3;
/// Hard character ceiling on a slug, so one very long word cannot widen a row.
pub const SLUG_MAX_CHARS: usize = 48;

/// How much of the input [`slug`] scans before giving up on finding three
/// meaningful words.
///
/// `slug` runs on every rail render against an untrusted harness/PTY title.
/// Without a scan bound, a title that is megabytes of filler ("so so so
/// so…") makes every render walk the whole string looking for words that
/// never arrive. This is generous enough to skip a realistic run of leading
/// filler while staying independent of how large the source title is.
const SLUG_SCAN_MAX_CHARS: usize = 512;

/// Filler a slug is better off without.
///
/// Prompts open with conversational scaffolding ("okay so can you please…"),
/// and taking the first three words verbatim would spend the whole slug on it.
/// Only words that never identify a session on their own are listed.
/// Contractions are folded to their letters before this list is consulted (see
/// [`APOSTROPHES`]), so the elided forms of the words above are listed too:
/// `let's` arrives here as `lets`, `I'm` as `im`.
const FILLER_WORDS: &[&str] = &[
    "a", "about", "an", "and", "are", "arent", "as", "at", "be", "but", "by", "can", "cant",
    "could", "couldnt", "do", "does", "doesnt", "dont", "for", "from", "hey", "hi", "how", "i",
    "id", "if", "ill", "im", "in", "into", "is", "isnt", "it", "its", "ive", "just", "let", "lets",
    "like", "me", "my", "of", "ok", "okay", "on", "or", "our", "please", "so", "thanks", "that",
    "thats", "the", "their", "then", "there", "theres", "these", "they", "theyre", "theyve",
    "this", "to", "uh", "um", "us", "was", "wasnt", "we", "well", "were", "weve", "what", "whats",
    "when", "which", "will", "with", "wont", "would", "wouldnt", "you", "youd", "youll", "youre",
    "youve", "your",
];

/// Apostrophes a contraction may be written with: ASCII, the typographic right
/// single quote most editors substitute, and the modifier letter Unicode
/// recommends for exactly this use.
///
/// These are dropped rather than treated as word breaks. Splitting on them
/// would cut `let's` into `let` and `s`, and the orphan `s` — meaningless on
/// its own, and not something a filler list can usefully enumerate — would
/// survive into the slug as its first word.
const APOSTROPHES: [char; 3] = ['\'', '\u{2019}', '\u{02bc}'];

/// Reduce any text to a session slug: at most [`SLUG_MAX_WORDS`] lowercase
/// words joined by hyphens, e.g. `fix-session-handoff`.
///
/// Everything that is not alphanumeric is a word break, which is also what
/// makes this safe for untrusted text: control bytes, escape sequences, and
/// newlines cannot survive into a rendered row.
///
/// Returns an empty string when no word survives — callers decide what an
/// unnameable session shows instead.
pub fn slug(text: &str) -> String {
    // Bound the scan itself, not just the output: split/lowercase/collect over
    // the whole string would let an unbounded title allocate and scan without
    // limit before the three-word cap below ever applies.
    let bounded = text.chars().take(SLUG_SCAN_MAX_CHARS);
    let bounded: String = bounded.collect();

    let mut first_words: Vec<String> = Vec::with_capacity(SLUG_MAX_WORDS);
    let mut meaningful: Vec<String> = Vec::with_capacity(SLUG_MAX_WORDS);
    for word in bounded.split(|c: char| !c.is_alphanumeric()) {
        if word.is_empty() {
            continue;
        }
        let word = word.to_lowercase();
        if first_words.len() < SLUG_MAX_WORDS {
            first_words.push(word.clone());
        }
        if meaningful.len() < SLUG_MAX_WORDS && !FILLER_WORDS.contains(&word.as_str()) {
            meaningful.push(word);
        }
        // Once three meaningful words are in hand nothing later in the title
        // can change the result, so stop walking it.
        if meaningful.len() == SLUG_MAX_WORDS {
            break;
        }
    }
    // All-filler input ("can you please") still gets a name, badly-but-stably,
    // rather than rendering as nothing at all.
    let source = if meaningful.is_empty() {
        first_words
    } else {
        meaningful
    };

    let mut slug = String::new();
    for word in source.into_iter().take(SLUG_MAX_WORDS) {
        let separator = usize::from(!slug.is_empty());
        let room = SLUG_MAX_CHARS.saturating_sub(slug.chars().count() + separator);
        if room == 0 {
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        // A single word longer than the ceiling is truncated rather than
        // dropped: dropping it can empty an otherwise usable slug.
        slug.extend(word.chars().take(room));
    }
    slug
}

/// Braille spinner frames shared by the app, login, and onboarding screens.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[cfg(test)]
#[path = "util_tests.rs"]
mod tests;
