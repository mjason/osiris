//! Canonical, lossless formatting for Osiris source.

use std::collections::BTreeMap;

use crate::{
    diagnostic::Diagnostic,
    reader,
    syntax::{Form, FormKind, Token, TokenKind, source_form_eq},
};

/// Version of the byte-level canonical formatting contract.
pub const FORMAT_VERSION: u32 = 4;

const MAX_LINE_WIDTH: usize = 80;
const METADATA_LINE_WIDTH: usize = 72;

#[derive(Clone, Copy, Debug)]
struct BreakSpec {
    anchor: usize,
    offset: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormatError {
    pub diagnostics: Vec<Diagnostic>,
}

/// Format one source snapshot without changing literal or comment contents.
pub fn format_source(source: &str) -> Result<String, FormatError> {
    let document = reader::read(source);
    if !document.diagnostics.is_empty() {
        return Err(FormatError {
            diagnostics: document.diagnostics,
        });
    }

    let layout = LayoutPlan::new(&document.forms, &document.tokens);
    let mut output = String::new();
    let mut depth = 0_usize;
    let mut line_start = true;
    let mut previous = None;
    let mut top_level = 0_usize;
    let mut column = 0_usize;
    let mut comment_since_top_level = false;
    let mut delimiters = Vec::new();
    let mut pending_indent = None;

    for token in &document.tokens {
        match token.kind {
            TokenKind::Whitespace => continue,
            TokenKind::Comment => {
                if depth == 0
                    && top_level > 0
                    && line_start
                    && !comment_since_top_level
                    && !output.ends_with("\n\n")
                {
                    output.push('\n');
                }
                if !line_start {
                    output.push(' ');
                } else if depth > 0 {
                    push_indent(&mut output, pending_indent.take().unwrap_or(depth * 2));
                }
                output.push_str(&token.text);
                output.push('\n');
                line_start = true;
                column = 0;
                previous = None;
                if depth == 0 {
                    comment_since_top_level = true;
                }
                continue;
            }
            _ => {}
        }

        let closing = is_closing(token.kind);
        if closing {
            depth = depth.saturating_sub(1);
            delimiters.pop();
        }
        let starts_top_level = document
            .forms
            .get(top_level)
            .is_some_and(|form| token.span.start == form.span.start);
        if starts_top_level && top_level > 0 && !comment_since_top_level {
            if !line_start {
                output.push('\n');
            }
            if !output.ends_with("\n\n") {
                output.push('\n');
            }
            line_start = true;
            column = 0;
            pending_indent = None;
        }
        if let Some(spec) = layout.breaks_before.get(&token.span.start)
            && !line_start
        {
            output.push('\n');
            line_start = true;
            column = 0;
            pending_indent = Some(resolve_indent(*spec, &delimiters));
        }
        let structural_break = !line_start
            && ((token.kind == TokenKind::Metadata
                && depth == 1
                && previous == Some(TokenKind::RightParen))
                || (token.kind == TokenKind::LeftParen && previous == Some(TokenKind::RightBrace)));
        if structural_break {
            output.push('\n');
            line_start = true;
            column = 0;
            pending_indent = None;
        }
        if line_start && depth > 0 {
            let indent = pending_indent.take().unwrap_or(depth * 2);
            push_indent(&mut output, indent);
            column = indent;
        }
        if !line_start && needs_space(previous, token.kind) {
            output.push(' ');
            column += 1;
        }
        let token_column = column;
        output.push_str(&token.text);
        column += token.text.chars().count();
        line_start = false;
        if is_opening(token.kind) {
            depth += 1;
            delimiters.push((token.span.start, token_column));
        }
        previous = Some(token.kind);

        if document
            .forms
            .get(top_level)
            .is_some_and(|form| token.span.end >= form.span.end)
        {
            output.push('\n');
            line_start = true;
            column = 0;
            pending_indent = None;
            previous = None;
            top_level += 1;
            comment_since_top_level = false;
        }
    }

    while output.ends_with('\n') {
        output.pop();
    }
    output.push('\n');

    let formatted = reader::read(&output);
    let equivalent = formatted.diagnostics.is_empty()
        && document.forms.len() == formatted.forms.len()
        && document
            .forms
            .iter()
            .zip(&formatted.forms)
            .all(|(left, right)| source_form_eq(left, right));
    if !equivalent {
        return Err(FormatError {
            diagnostics: vec![Diagnostic::error(
                "OSR-F0001",
                "formatter could not prove that the formatted source preserves reader meaning",
                Default::default(),
            )],
        });
    }
    Ok(output)
}

#[derive(Default)]
struct LayoutPlan {
    breaks_before: BTreeMap<usize, BreakSpec>,
}

impl LayoutPlan {
    fn new(forms: &[Form], tokens: &[Token]) -> Self {
        let mut plan = Self::default();
        for form in forms {
            plan.visit(form, tokens);
        }
        plan
    }

    fn visit(&mut self, form: &Form, tokens: &[Token]) {
        self.visit_metadata(form, tokens);
        match &form.kind {
            FormKind::List(items) => {
                self.plan_list(form, items, tokens);
                for item in items {
                    self.visit(item, tokens);
                }
            }
            FormKind::Vector(items) | FormKind::Set(items) => {
                self.plan_sequential_collection(form, items, tokens);
                for item in items {
                    self.visit(item, tokens);
                }
            }
            FormKind::Map(items) => {
                self.plan_map(form, items, tokens);
                for item in items {
                    self.visit(item, tokens);
                }
            }
            FormKind::ReaderMacro { form, .. } => self.visit(form, tokens),
            _ => {}
        }
    }

    fn visit_metadata(&mut self, form: &Form, tokens: &[Token]) {
        let metadata_anchor = form.metadata.first().and_then(|entry| {
            enclosing_delimiter(tokens, entry.key.span.start, TokenKind::LeftBrace)
        });
        if form.metadata.len() > 1
            && (form.metadata.len() > 2
                || flat_width(form.span.start, form.datum_span.start, tokens) > METADATA_LINE_WIDTH)
        {
            if let Some(anchor) = metadata_anchor {
                for entry in form.metadata.iter().skip(1) {
                    self.add_break(entry.key.span.start, anchor, 1);
                }
            }
        }
        for entry in &form.metadata {
            if (form_symbol(&entry.key) == Some(":doc")
                || matches!(&entry.key.kind, FormKind::Keyword(name) if name.canonical.trim_start_matches(':') == "doc"))
                && matches!(entry.value.kind, FormKind::Map(_))
                && let Some(anchor) = metadata_anchor
            {
                self.add_break(entry.value.span.start, anchor, 1);
            }
            self.visit(&entry.key, tokens);
            self.visit(&entry.value, tokens);
        }
    }

    fn plan_map(&mut self, form: &Form, items: &[Form], tokens: &[Token]) {
        if items.len() < 4 {
            return;
        }
        if items.len() > 4 || flat_form_width(form, tokens) > METADATA_LINE_WIDTH {
            for key in items.iter().step_by(2).skip(1) {
                self.add_break(key.span.start, form.datum_span.start, 1);
            }
        }
    }

    fn plan_list(&mut self, form: &Form, items: &[Form], tokens: &[Token]) {
        let Some(head) = items.first().and_then(form_symbol) else {
            return;
        };
        let width = flat_form_width(form, tokens);
        match head {
            "extern" => {
                for item in items.iter().skip(3) {
                    self.add_break(item.span.start, form.datum_span.start, 2);
                }
            }
            "export" => {
                if let Some(exports) = items.get(1) {
                    self.add_break(exports.span.start, form.datum_span.start, 2);
                }
            }
            "defn" | "defmacro" | "defn-for-syntax" | "defstruct" => {
                if let Some(parameters) = items.get(2)
                    && flat_width(form.datum_span.start, parameters.span.end, tokens)
                        > MAX_LINE_WIDTH
                {
                    self.add_break(parameters.span.start, form.datum_span.start, 2);
                }
                for body in items.iter().skip(3) {
                    self.add_break(body.span.start, form.datum_span.start, 2);
                }
            }
            "let" | "letfn" | "loop" | "for" | "forv" | "doseq" | "dotimes" | "binding"
            | "with-open" | "when-let" | "when-some" | "if-let" | "if-some" => {
                if let Some(bindings) = items.get(1) {
                    self.plan_binding_vector(bindings, tokens);
                }
                for body in items.iter().skip(2) {
                    self.add_break(body.span.start, form.datum_span.start, 2);
                }
            }
            "if" | "if-not" => {
                for branch in items.iter().skip(2) {
                    self.add_break(branch.span.start, form.datum_span.start, 2);
                }
            }
            "when" | "when-not" => {
                for body in items.iter().skip(2) {
                    self.add_break(body.span.start, form.datum_span.start, 2);
                }
            }
            "do" | "try" | "comment" => {
                for body in items.iter().skip(1) {
                    self.add_break(body.span.start, form.datum_span.start, 2);
                }
            }
            "cond" => self.plan_pairs(form, items, 1),
            "case" => self.plan_pairs(form, items, 2),
            "condp" => self.plan_pairs(form, items, 3),
            "->" | "->>" | "some->" | "some->>" => {
                let offset = head.chars().count() + 2;
                for step in items.iter().skip(2) {
                    self.add_break(step.span.start, form.datum_span.start, offset);
                }
            }
            "cond->" | "cond->>" => self.plan_pairs(form, items, 2),
            "as->" => {
                for body in items.iter().skip(3) {
                    self.add_break(body.span.start, form.datum_span.start, 2);
                }
            }
            "doto" => {
                for body in items.iter().skip(2) {
                    self.add_break(body.span.start, form.datum_span.start, 2);
                }
            }
            _ if width > MAX_LINE_WIDTH => {
                let offset = head.chars().count() + 2;
                for argument in items.iter().skip(2) {
                    self.add_break(argument.span.start, form.datum_span.start, offset);
                }
            }
            _ => {}
        }
    }

    fn plan_binding_vector(&mut self, form: &Form, tokens: &[Token]) {
        let FormKind::Vector(items) = &form.kind else {
            return;
        };
        if items.len() > 2 || flat_form_width(form, tokens) > MAX_LINE_WIDTH / 2 {
            for binding in items.iter().step_by(2).skip(1) {
                self.add_break(binding.span.start, form.datum_span.start, 1);
            }
        }
    }

    fn plan_sequential_collection(&mut self, form: &Form, items: &[Form], tokens: &[Token]) {
        if items.len() < 2 || flat_form_width(form, tokens) <= MAX_LINE_WIDTH {
            return;
        }
        let mut line_width = 1_usize;
        for (index, item) in items.iter().enumerate() {
            let item_width = flat_width(item.span.start, item.span.end, tokens);
            let separator = usize::from(index > 0);
            if index > 0
                && line_width
                    .saturating_add(separator)
                    .saturating_add(item_width)
                    > 72
            {
                self.add_break(item.span.start, form.datum_span.start, 1);
                line_width = 1_usize.saturating_add(item_width);
            } else {
                line_width = line_width
                    .saturating_add(separator)
                    .saturating_add(item_width);
            }
        }
    }

    fn plan_pairs(&mut self, form: &Form, items: &[Form], first: usize) {
        for test in items.iter().skip(first).step_by(2) {
            self.add_break(test.span.start, form.datum_span.start, 2);
        }
    }

    fn add_break(&mut self, position: usize, anchor: usize, offset: usize) {
        self.breaks_before
            .insert(position, BreakSpec { anchor, offset });
    }
}

fn form_symbol(form: &Form) -> Option<&str> {
    match &form.kind {
        FormKind::Symbol(name) => Some(name.canonical.as_str()),
        _ => None,
    }
}

fn flat_form_width(form: &Form, tokens: &[Token]) -> usize {
    flat_width(form.datum_span.start, form.datum_span.end, tokens)
}

fn flat_width(start: usize, end: usize, tokens: &[Token]) -> usize {
    let mut width = 0;
    let mut previous = None;
    for token in tokens
        .iter()
        .filter(|token| token.span.start >= start && token.span.end <= end)
    {
        if token.kind == TokenKind::Whitespace {
            continue;
        }
        if token.kind == TokenKind::Comment {
            return usize::MAX;
        }
        if needs_space(previous, token.kind) {
            width += 1;
        }
        width += token.text.chars().count();
        previous = Some(token.kind);
    }
    width
}

fn resolve_indent(spec: BreakSpec, delimiters: &[(usize, usize)]) -> usize {
    delimiters
        .iter()
        .rev()
        .find_map(|(position, column)| (*position == spec.anchor).then_some(*column + spec.offset))
        .unwrap_or(spec.offset)
}

fn enclosing_delimiter(tokens: &[Token], position: usize, kind: TokenKind) -> Option<usize> {
    let mut stack = Vec::new();
    for token in tokens
        .iter()
        .take_while(|token| token.span.start < position)
    {
        if is_opening(token.kind) {
            stack.push((token.kind, token.span.start));
        } else if is_closing(token.kind) {
            stack.pop();
        }
    }
    stack
        .iter()
        .rev()
        .find_map(|(candidate, position)| (*candidate == kind).then_some(*position))
}

fn push_indent(output: &mut String, spaces: usize) {
    for _ in 0..spaces {
        output.push(' ');
    }
}

const fn is_opening(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LeftParen | TokenKind::LeftBracket | TokenKind::LeftBrace | TokenKind::SetStart
    )
}

const fn is_closing(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace
    )
}

const fn is_prefix(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Quote
            | TokenKind::SyntaxQuote
            | TokenKind::Unquote
            | TokenKind::UnquoteSplicing
            | TokenKind::Metadata
    )
}

const fn needs_space(previous: Option<TokenKind>, current: TokenKind) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    !is_opening(previous) && !is_prefix(previous) && !is_closing(current)
}

#[cfg(test)]
mod tests;
