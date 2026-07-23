use super::*;

pub(super) fn node_id_for_span(document: &OpenDocument, span: Span) -> Option<u64> {
    document
        .analysis
        .document
        .nodes
        .iter()
        .filter(|node| node.span.start <= span.start && span.end <= node.span.end)
        .min_by(|left, right| {
            let left_width = left.span.end.saturating_sub(left.span.start);
            let right_width = right.span.end.saturating_sub(right.span.start);
            left_width
                .cmp(&right_width)
                .then_with(|| right.path.segments().len().cmp(&left.path.segments().len()))
                .then_with(|| left.span.start.cmp(&right.span.start))
                .then_with(|| left.id.cmp(&right.id))
        })
        .map(|node| node.id.get())
}

pub(super) fn escape_markdown(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('*', "\\*")
        .replace('_', "\\_")
}

pub(super) fn apply_content_change(
    source: &mut String,
    change: &TextDocumentContentChangeEvent,
) -> Result<(), LspStateError> {
    let Some(range) = change.range else {
        source.clone_from(&change.text);
        return Ok(());
    };
    let Some(start) = position_to_offset(source, range.start) else {
        return Err(LspStateError::new(
            INVALID_PARAMS,
            "change start is outside the document",
        ));
    };
    let Some(end) = position_to_offset(source, range.end) else {
        return Err(LspStateError::new(
            INVALID_PARAMS,
            "change end is outside the document",
        ));
    };
    if start > end || !source.is_char_boundary(start) || !source.is_char_boundary(end) {
        return Err(LspStateError::new(
            INVALID_PARAMS,
            "change range is not a valid UTF-8 boundary",
        ));
    }
    source.replace_range(start..end, &change.text);
    Ok(())
}

/// Converts an LSP UTF-16 position to a UTF-8 byte offset.
#[must_use]
pub fn position_to_offset(source: &str, position: Position) -> Option<usize> {
    let mut line = 0_u32;
    let mut line_start = 0_usize;
    for (offset, byte) in source.bytes().enumerate() {
        if line == position.line {
            break;
        }
        if byte == b'\n' {
            line += 1;
            line_start = offset + 1;
        }
    }
    if line != position.line {
        return None;
    }
    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |relative| line_start + relative);
    let line_text = source[line_start..line_end]
        .strip_suffix('\r')
        .unwrap_or(&source[line_start..line_end]);
    let mut utf16 = 0_u32;
    for (relative, character) in line_text.char_indices() {
        if utf16 == position.character {
            return Some(line_start + relative);
        }
        let width = character.len_utf16() as u32;
        if utf16 + width > position.character {
            return None;
        }
        utf16 += width;
    }
    (utf16 == position.character).then_some(line_start + line_text.len())
}

/// Converts a UTF-8 byte offset to an LSP UTF-16 position.
#[must_use]
pub fn offset_to_position(source: &str, offset: usize) -> Position {
    let offset = offset.min(source.len());
    let offset = if source.is_char_boundary(offset) {
        offset
    } else {
        (0..offset)
            .rev()
            .find(|candidate| source.is_char_boundary(*candidate))
            .unwrap_or(0)
    };
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let character = source[line_start..offset].encode_utf16().count() as u32;
    Position { line, character }
}

#[must_use]
pub fn span_to_range(source: &str, span: Span) -> Range {
    Range {
        start: offset_to_position(source, span.start),
        end: offset_to_position(source, span.end),
    }
}
