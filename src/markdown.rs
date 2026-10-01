//! Markdown structure every scanner shares: which lines are prose, and which
//! parts of a prose line are inline code.
//!
//! Five scanners grew the same fence toggle independently — `claudemd` twice,
//! `links`, `pointers`, `ledger` — and the `CLAUDE.md` import resolver would
//! have been the sixth. A rule copied six times is a rule that drifts six
//! ways: one copy learns that a closing fence must match its opener and the
//! others do not, and two gates disagree about the same file. So the rule lives
//! here, once, and every scanner asks it.
//!
//! The rule is deliberately the one all five copies already agreed on: a line
//! whose first non-blank characters are ```` ``` ```` or `~~~` opens or closes a
//! fence, with no attempt to match the closer's character or length against the
//! opener. Tightening it is a behaviour change for every gate at once and earns
//! its own commit and its own fixtures; it is not something to slip in under a
//! refactor.

/// Where a line sits relative to fenced code blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// A ```` ``` ```` or `~~~` delimiter line, opening or closing a fence.
    Fence,
    /// Inside a fenced block: shown, not written. A heading, bullet, waiver or
    /// import written here is an EXAMPLE of one, never one.
    Fenced,
    /// Outside every fence.
    Prose,
}

/// Line-by-line fence tracker.
///
/// Stateful rather than an iterator adaptor because the scanners walk different
/// shapes — a whole document, a section slice, a body after frontmatter — and
/// some of them (the `claudemd` census) must still see fenced lines. Each one
/// asks per line and decides for itself what a non-prose line means.
#[derive(Debug, Clone, Copy, Default)]
pub struct Fences {
    fenced: bool,
}

impl Fences {
    /// A tracker positioned before the first line, outside any fence.
    #[must_use]
    pub fn new() -> Self { Self::default() }

    /// Classify the next line, advancing the fence state.
    pub fn classify(&mut self, line: &str) -> LineKind {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            self.fenced = !self.fenced;
            LineKind::Fence
        } else if self.fenced {
            LineKind::Fenced
        } else {
            LineKind::Prose
        }
    }
}

/// Every prose line of `content` with its zero-based line index.
pub fn prose_lines(content: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut fences = Fences::new();
    content
        .lines()
        .enumerate()
        .filter(move |(_, line)| fences.classify(line) == LineKind::Prose)
}

/// One piece of a prose line, split at backticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment<'a> {
    /// Text outside inline code.
    Prose {
        /// The text.
        text: &'a str,
        /// `true` only for the segment that begins the line. Every later prose
        /// segment begins immediately after a CLOSING backtick, so a token at
        /// its offset 0 is butted against code rather than standing alone —
        /// which is the difference between `` `X`/y `` and a real `/y`.
        at_line_start: bool,
    },
    /// The inside of an inline code span, delimiters excluded.
    Code(&'a str),
}

/// Split one line into prose and inline-code segments.
///
/// Odd segments of a backtick split are inline code, even segments prose. A
/// span carries its own delimiters, so "is this inside code?" is answerable per
/// line without a markdown parser; a span broken across lines is the one shape
/// this misreads, and none of the scanned corpora write one.
pub fn segments(line: &str) -> impl Iterator<Item = Segment<'_>> {
    line.split('`').enumerate().map(|(index, text)| {
        if index % 2 == 1 {
            Segment::Code(text)
        } else {
            Segment::Prose { text, at_line_start: index == 0 }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_fence_spellings_open_and_close() {
        let mut f = Fences::new();
        let kinds: Vec<LineKind> =
            ["a", "```rust", "b", "```", "c", "  ~~~", "d", "~~~", "e"].iter().map(|l| f.classify(l)).collect();
        assert_eq!(
            kinds,
            [
                LineKind::Prose,
                LineKind::Fence,
                LineKind::Fenced,
                LineKind::Fence,
                LineKind::Prose,
                LineKind::Fence,
                LineKind::Fenced,
                LineKind::Fence,
                LineKind::Prose,
            ]
        );
    }

    #[test]
    fn prose_lines_keep_their_original_index() {
        let got: Vec<(usize, &str)> = prose_lines("a\n```\nb\n```\nc\n").collect();
        assert_eq!(got, [(0, "a"), (4, "c")]);
    }

    #[test]
    fn segments_alternate_prose_and_code() {
        let got: Vec<Segment<'_>> = segments("x `y` z").collect();
        assert_eq!(
            got,
            [
                Segment::Prose { text: "x ", at_line_start: true },
                Segment::Code("y"),
                Segment::Prose { text: " z", at_line_start: false },
            ]
        );
    }
}
