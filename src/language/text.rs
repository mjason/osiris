//! Column measurement for source text shown to a person.
//!
//! Two different questions need different answers, so they are two functions.
//!
//! [`canonical_width`] is what the formatter asks: how wide is this text in the
//! one canonical format? It must not depend on the environment, otherwise
//! `osr fmt --check` would pass on one machine and fail on another.
//!
//! [`terminal_width`] is what a diagnostic asks: how many columns will this
//! occupy in the reader's terminal right now? That genuinely depends on the
//! terminal's font, which no process can detect, so it is configurable.

use std::sync::OnceLock;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Columns a tab advances to, matching the near-universal terminal default.
pub const TAB_WIDTH: usize = 8;

/// Selects how East Asian Ambiguous characters are measured for display.
///
/// Unicode Annex #11 leaves the width of characters such as `→`, `※`, `±`, and
/// the box-drawing set to context: one column beside Latin text, two beside CJK
/// text. A terminal resolves this by its font, and there is no escape sequence
/// to ask it, so the reader tells us instead.
#[must_use]
pub fn ambiguous_is_wide() -> bool {
    static WIDE: OnceLock<bool> = OnceLock::new();
    *WIDE.get_or_init(|| {
        // Deliberately not inferred from LANG or LC_CTYPE. Plenty of people
        // read CJK source under an en_US.UTF-8 locale with a CJK font, and
        // guessing wrong is worse than a stable default.
        std::env::var("OSIRIS_EAST_ASIAN_WIDTH")
            .is_ok_and(|value| value.trim().eq_ignore_ascii_case("wide"))
    })
}

/// Display columns used by the canonical source format.
///
/// Ambiguous characters count as one column, which is Annex #11's
/// recommendation when the context is unknown. This is fixed on purpose: the
/// canonical format is a compiler contract, not a presentation choice.
///
/// The text must not contain tabs; run [`expand_tabs`] first if it might.
#[must_use]
pub fn canonical_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Display columns the text is expected to occupy in the reader's terminal.
///
/// The text must not contain tabs; run [`expand_tabs`] first if it might.
#[must_use]
pub fn terminal_width(text: &str) -> usize {
    if ambiguous_is_wide() {
        UnicodeWidthStr::width_cjk(text)
    } else {
        UnicodeWidthStr::width(text)
    }
}

fn character_terminal_width(character: char) -> usize {
    let width = if ambiguous_is_wide() {
        UnicodeWidthChar::width_cjk(character)
    } else {
        UnicodeWidthChar::width(character)
    };
    // A control character has no width of its own. Tabs are the one case that
    // matters and `expand_tabs` has already replaced them.
    width.unwrap_or(0)
}

/// Replaces tabs with the spaces a terminal would advance over.
///
/// Osiris canonical source has no tabs, but the reader accepts them and a
/// diagnostic may be rendered before the file was ever formatted. A tab's width
/// depends on the column it starts at, so it cannot be handled by a width
/// function alone; both the quoted source line and the caret line have to be
/// expanded for the two to agree.
#[must_use]
pub fn expand_tabs(text: &str) -> String {
    if !text.contains('\t') {
        return text.to_owned();
    }
    let mut expanded = String::with_capacity(text.len());
    let mut column = 0;
    for character in text.chars() {
        if character == '\t' {
            let advance = TAB_WIDTH - (column % TAB_WIDTH);
            expanded.extend(std::iter::repeat_n(' ', advance));
            column += advance;
        } else {
            expanded.push(character);
            column += character_terminal_width(character);
        }
    }
    expanded
}

/// A slice of one source line chosen to keep a span visible in a terminal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineWindow {
    /// The visible text, already tab-expanded.
    pub text: String,
    /// Display column the slice starts at, zero-based, for caret placement.
    pub start_column: usize,
    pub elided_before: bool,
    pub elided_after: bool,
}

/// Chooses the part of a line to quote so that `[focus_start, focus_end)`
/// stays visible within `budget` columns.
///
/// Without this a long line soft-wraps in the terminal and the caret lands
/// under the wrong visual row, which makes the whole diagnostic misleading.
/// A CJK line reaches that point at half the character count of a Latin one.
#[must_use]
pub fn line_window(line: &str, focus_start: usize, focus_end: usize, budget: usize) -> LineWindow {
    let line = expand_tabs(line);
    let total = terminal_width(&line);
    if total <= budget {
        return LineWindow {
            text: line,
            start_column: 0,
            elided_before: false,
            elided_after: false,
        };
    }

    // Leave room for the ellipsis markers the window may need on either side.
    const MARKER: &str = "...";
    let marker_width = terminal_width(MARKER);
    let focus_width = focus_end.saturating_sub(focus_start);
    let inner = budget
        .saturating_sub(marker_width * 2)
        .max(focus_width.max(1));

    // Centre the focus, then clamp so a span near either end shows its context.
    let half = inner.saturating_sub(focus_width) / 2;
    let mut start = focus_start.saturating_sub(half);
    if start + inner > total {
        start = total.saturating_sub(inner);
    }

    let (before, rest) = split_at_column(&line, start);
    let (inside, after) = split_at_column(rest, inner);
    let elided_before = !before.is_empty();
    let elided_after = !after.is_empty();
    let mut text = String::new();
    if elided_before {
        text.push_str(MARKER);
    }
    text.push_str(inside);
    if elided_after {
        text.push_str(MARKER);
    }
    LineWindow {
        text,
        // The rendered slice gains the leading marker, so the column the caret
        // must be offset by is the elided width minus what the marker occupies.
        start_column: terminal_width(before)
            .saturating_sub(usize::from(elided_before) * marker_width),
        elided_before,
        elided_after,
    }
}

/// Splits at the last character boundary whose display width fits `columns`.
///
/// A zero-width character belongs to the character it modifies, so it is never
/// separated from it.
fn split_at_column(text: &str, columns: usize) -> (&str, &str) {
    let mut width = 0;
    let mut boundary = 0;
    for (offset, character) in text.char_indices() {
        let character_width = character_terminal_width(character);
        if character_width > 0 && width + character_width > columns {
            return text.split_at(boundary);
        }
        width += character_width;
        boundary = offset + character.len_utf8();
    }
    (text, "")
}

#[cfg(test)]
#[path = "text/tests.rs"]
mod tests;
