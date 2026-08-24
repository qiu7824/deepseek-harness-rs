//! A dependency-light retention library: bounded model-facing output for
//! tools that must cap how much context they return. Rust port of
//! `@deepseek-ai/dsh-output-retention`.

use std::collections::VecDeque;

/// How much content the retainer omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Omitted {
    None,
    Exact { count: usize },
    Unknown,
}

/// The caller receives this after each `push()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushDecision {
    /// Was this whole unit / all of this chunk's bytes retained?
    pub kept: bool,
    /// Cumulative: has the retainer omitted anything due to the budget yet?
    pub truncated: bool,
}

/// Final result for ordered logical units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedItems<T> {
    pub items: Vec<T>,
    pub truncated: bool,
    pub seen: usize,
    pub kept: usize,
    pub omitted: Omitted,
}

/// Final result for text streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedText {
    pub text: String,
    pub truncated: bool,
    pub omitted_bytes: Omitted,
}

/// Item retention strategy (head only in v1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemRetentionStrategy {
    pub max_items: usize,
}

/// Text retention strategy: keep a prefix, a suffix, or both, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRetentionStrategy {
    Head {
        max_bytes: usize,
    },
    Tail {
        max_bytes: usize,
    },
    HeadTail {
        head_bytes: usize,
        tail_bytes: usize,
    },
}

/// A neutral, tool-agnostic description of one retention outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionNotice {
    pub scope: String,
    pub strategy: NoticeStrategy,
    pub unit: NoticeUnit,
    pub limit: NoticeLimit,
    pub kept: usize,
    pub omitted: Omitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeStrategy {
    Head,
    Tail,
    HeadTail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeUnit {
    Items,
    Bytes,
    Chars,
    Lines,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLimit {
    Single(usize),
    HeadTail { head: usize, tail: usize },
}

fn assert_budget(value: usize, name: &str) {
    // usize is always non-negative; the integer check exists only for the
    // TS-parity panic message shape.
    let _ = (value, name);
}

/// Bounds an ordered stream of logical units, keeping the first `max_items`
/// (TS `ItemRetainer`).
pub struct ItemRetainer<T> {
    max_items: usize,
    items: Vec<T>,
    seen: usize,
    omitted_count: usize,
}

impl<T> ItemRetainer<T> {
    pub fn new(strategy: ItemRetentionStrategy) -> Self {
        assert_budget(strategy.max_items, "maxItems");
        Self {
            max_items: strategy.max_items,
            items: Vec::new(),
            seen: 0,
            omitted_count: 0,
        }
    }

    pub fn push(&mut self, item: T) -> PushDecision {
        self.seen += 1;
        if self.items.len() < self.max_items {
            self.items.push(item);
            return PushDecision {
                kept: true,
                truncated: false,
            };
        }
        self.omitted_count += 1;
        PushDecision {
            kept: false,
            truncated: true,
        }
    }

    pub fn finish(self) -> RetainedItems<T> {
        let truncated = self.omitted_count > 0;
        RetainedItems {
            kept: self.items.len(),
            items: self.items,
            truncated,
            seen: self.seen,
            omitted: if truncated {
                Omitted::Exact {
                    count: self.omitted_count,
                }
            } else {
                Omitted::None
            },
        }
    }
}

fn concat(chunks: &VecDeque<Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::with_capacity(chunks.iter().map(|c| c.len()).sum());
    for chunk in chunks {
        out.extend_from_slice(chunk);
    }
    out
}

/// Drop a trailing incomplete UTF-8 sequence so a prefix cut never emits a
/// replacement char at the boundary (TS `trimTrailingPartialUtf8`).
fn trim_trailing_partial_utf8(bytes: &[u8]) -> &[u8] {
    let mut i = bytes.len();
    while i > 0 && (bytes[i - 1] & 0xc0) == 0x80 && bytes.len() - i <= 3 {
        i -= 1;
    }
    if i == 0 {
        return bytes;
    }
    let lead = bytes[i - 1];
    let expected = if lead < 0x80 {
        1
    } else if lead < 0xe0 {
        2
    } else if lead < 0xf0 {
        3
    } else if lead < 0xf8 {
        4
    } else {
        0
    };
    if expected == 0 {
        return bytes;
    }
    if bytes.len() - i + 1 < expected {
        &bytes[..i - 1]
    } else {
        bytes
    }
}

/// Drop leading continuation bytes so a suffix cut starts on a lead/ASCII
/// byte (TS `trimLeadingContinuationUtf8`).
fn trim_leading_continuation_utf8(bytes: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < bytes.len() && (bytes[i] & 0xc0) == 0x80 {
        i += 1;
    }
    &bytes[i..]
}

/// Bounds a byte-oriented text stream, keeping a prefix, a suffix, or both
/// (TS `TextRetainer`).
pub struct TextRetainer {
    prefix_cap: usize,
    suffix_cap: usize,
    prefix_chunks: VecDeque<Vec<u8>>,
    prefix_held: usize,
    suffix_chunks: VecDeque<Vec<u8>>,
    suffix_held: usize,
    total: usize,
}

impl TextRetainer {
    pub fn new(strategy: TextRetentionStrategy) -> Self {
        let (prefix_cap, suffix_cap) = match strategy {
            TextRetentionStrategy::Head { max_bytes } => {
                assert_budget(max_bytes, "maxBytes");
                (max_bytes, 0)
            }
            TextRetentionStrategy::Tail { max_bytes } => {
                assert_budget(max_bytes, "maxBytes");
                (0, max_bytes)
            }
            TextRetentionStrategy::HeadTail {
                head_bytes,
                tail_bytes,
            } => {
                assert_budget(head_bytes, "headBytes");
                assert_budget(tail_bytes, "tailBytes");
                (head_bytes, tail_bytes)
            }
        };
        Self {
            prefix_cap,
            suffix_cap,
            prefix_chunks: VecDeque::new(),
            prefix_held: 0,
            suffix_chunks: VecDeque::new(),
            suffix_held: 0,
            total: 0,
        }
    }

    /// Bytes omitted once `total` bytes have been seen.
    fn omitted_at(&self, total: usize) -> usize {
        let prefix_len = total.min(self.prefix_cap);
        let suffix_len = (total - prefix_len).min(self.suffix_cap);
        total - prefix_len - suffix_len
    }

    pub fn push(&mut self, chunk: &[u8]) -> PushDecision {
        let before = self.total;
        self.total += chunk.len();

        let room = self.prefix_cap.saturating_sub(self.prefix_held);
        let take = room.min(chunk.len());
        if take > 0 {
            self.prefix_chunks.push_back(chunk[..take].to_vec());
            self.prefix_held += take;
        }

        if self.suffix_cap > 0 {
            self.suffix_chunks.push_back(chunk.to_vec());
            self.suffix_held += chunk.len();
            while let Some(head) = self.suffix_chunks.front() {
                if self.suffix_held.saturating_sub(head.len()) < self.suffix_cap {
                    break;
                }
                let head = self.suffix_chunks.pop_front().expect("front existed");
                self.suffix_held -= head.len();
            }
            if self.suffix_held > self.suffix_cap {
                let excess = self.suffix_held - self.suffix_cap;
                if let Some(head) = self.suffix_chunks.front_mut() {
                    // TS: `head.subarray(excess)` — drop the first `excess`
                    // leading bytes of the head chunk (head.len() > excess by
                    // the loop invariant above).
                    let trimmed = head.split_off(excess.min(head.len()));
                    *head = trimmed;
                }
                self.suffix_held -= excess;
            }
        }

        let dropped_this_chunk = self.omitted_at(self.total) > self.omitted_at(before);
        PushDecision {
            kept: !dropped_this_chunk,
            truncated: self.omitted_at(self.total) > 0,
        }
    }

    pub fn finish(self) -> RetainedText {
        let prefix_len = self.total.min(self.prefix_cap);
        let suffix_len = (self.total - prefix_len).min(self.suffix_cap);

        let prefix = concat(&self.prefix_chunks);
        let suffix_all = concat(&self.suffix_chunks);
        let suffix_start = self.suffix_held.saturating_sub(suffix_len);
        let suffix = &suffix_all[suffix_start..];

        // With nothing omitted by budget, prefix and suffix are adjacent
        // slices of one stream (no real cut). Only a real omitted gap makes
        // each side a true cut: trim each to a UTF-8 boundary.
        let budget_omitted = self.omitted_at(self.total);
        let (trimmed_prefix, trimmed_suffix): (Vec<u8>, Vec<u8>) = if budget_omitted > 0 {
            (
                trim_trailing_partial_utf8(&prefix).to_vec(),
                trim_leading_continuation_utf8(suffix).to_vec(),
            )
        } else {
            (prefix, suffix.to_vec())
        };
        let omitted = self.total - trimmed_prefix.len() - trimmed_suffix.len();
        let truncated = omitted > 0;
        let mut joined = trimmed_prefix;
        joined.extend_from_slice(&trimmed_suffix);
        let text = String::from_utf8_lossy(&joined).into_owned();

        RetainedText {
            text,
            truncated,
            omitted_bytes: if truncated {
                Omitted::Exact { count: omitted }
            } else {
                Omitted::None
            },
        }
    }
}

/// Standardized, false-precision-safe wording for one [`Omitted`] value
/// (TS `describeOmitted`).
pub fn describe_omitted(omitted: Omitted, unit: NoticeUnit) -> String {
    let noun = match unit {
        NoticeUnit::Items => "items",
        NoticeUnit::Bytes => "bytes",
        NoticeUnit::Chars => "chars",
        NoticeUnit::Lines => "lines",
    };
    match omitted {
        Omitted::None => String::new(),
        Omitted::Exact { count } => format!("Omitted {count} {noun}."),
        Omitted::Unknown => format!("More {noun} were omitted."),
    }
}

/// Turn a [`RetentionNotice`] into a one-line footer (TS
/// `formatRetentionNotice`).
pub fn format_retention_notice(
    notice: &RetentionNotice,
    recovery: impl Fn(&RetentionNotice) -> String,
) -> String {
    [
        describe_omitted(notice.omitted, notice.unit),
        recovery(notice),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ")
}
