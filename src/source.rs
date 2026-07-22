use serde::Serialize;

/// A half-open UTF-8 byte range in a source file.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    #[must_use]
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    #[must_use]
    pub const fn empty(offset: usize) -> Self {
        Self::new(offset, offset)
    }

    #[must_use]
    pub fn cover(self, other: Self) -> Self {
        Self::new(self.start.min(other.start), self.end.max(other.end))
    }
}

/// Maps byte offsets to one-based source locations.
#[derive(Clone, Debug)]
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    #[must_use]
    pub fn new(source: &str) -> Self {
        let mut starts = vec![0];
        let bytes = source.as_bytes();
        let mut offset = 0;
        while offset < bytes.len() {
            match bytes[offset] {
                b'\r' if bytes.get(offset + 1) == Some(&b'\n') => {
                    offset += 2;
                    starts.push(offset);
                }
                b'\r' | b'\n' => {
                    offset += 1;
                    starts.push(offset);
                }
                _ => offset += 1,
            }
        }
        Self { starts }
    }

    #[must_use]
    pub fn line_column(&self, source: &str, offset: usize) -> (usize, usize) {
        let offset = offset.min(source.len());
        let line = self.starts.partition_point(|start| *start <= offset) - 1;
        let column = source[self.starts[line]..offset].chars().count();
        (line + 1, column + 1)
    }

    #[must_use]
    pub fn line_bounds(&self, source: &str, one_based_line: usize) -> Span {
        let line = one_based_line.saturating_sub(1).min(self.starts.len() - 1);
        let start = self.starts[line];
        let mut end = self.starts.get(line + 1).copied().unwrap_or(source.len());
        if end > start && source.as_bytes().get(end - 1) == Some(&b'\n') {
            end -= 1;
        }
        if end > start && source.as_bytes().get(end - 1) == Some(&b'\r') {
            end -= 1;
        }
        Span::new(start, end)
    }
}

#[cfg(test)]
mod tests {
    use super::LineIndex;

    #[test]
    fn locations_count_unicode_characters_not_bytes() {
        let source = "ab\n中x";
        let index = LineIndex::new(source);
        assert_eq!(index.line_column(source, 3), (2, 1));
        assert_eq!(index.line_column(source, 6), (2, 2));
    }

    #[test]
    fn recognizes_lf_crlf_and_cr_line_endings() {
        let source = "a\nb\r\nc\rd";
        let index = LineIndex::new(source);

        assert_eq!(index.line_column(source, 2), (2, 1));
        assert_eq!(index.line_column(source, 5), (3, 1));
        assert_eq!(index.line_column(source, 7), (4, 1));
    }
}
