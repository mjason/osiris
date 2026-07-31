//! Canonical, lossless formatting for Osiris source.

use std::collections::BTreeMap;

use crate::{
    diagnostic::Diagnostic,
    reader,
    syntax::{Document, Form, FormKind, Token, TokenKind, source_form_eq},
};

/// Version of the byte-level canonical formatting contract.
pub const FORMAT_VERSION: u32 = 8;

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

/// Rounds of plan-then-emit before the layout is taken as it stands.
///
/// Each round's plan is a pure function of the form structure and the columns
/// the previous round measured, so the sequence is deterministic. It normally
/// settles on the second round; the cap only bounds a pathological oscillation.
const LAYOUT_ROUNDS: usize = 3;

/// One emitted layout, with the column every form's opening delimiter landed
/// in, keyed by that form's `datum_span.start`.
struct Emitted {
    text: String,
    anchors: BTreeMap<usize, usize>,
    /// Byte ranges of `text` holding an embedded language's own canonical
    /// layout. Those lines answer to that language's formatter, not this one.
    foreign: Vec<std::ops::Range<usize>>,
}

/// Format one source snapshot without changing literal or comment contents.
pub fn format_source(source: &str) -> Result<String, FormatError> {
    let document = reader::read(source);
    if !document.diagnostics.is_empty() {
        return Err(FormatError {
            diagnostics: document.diagnostics,
        });
    }

    // The planner chooses breaks from form structure and flat widths, but the
    // column a break lands in is only known once the text is emitted. Feeding
    // the measured columns back and re-planning applies the line-width budget
    // where the text actually sits. The gap between the two matters most for
    // CJK source: head-aligned indents are deeper and every identifier
    // character costs two columns, so a plan made from column zero overflows.
    let mut anchors = BTreeMap::new();
    let mut output = String::new();
    for round in 0..LAYOUT_ROUNDS {
        let layout = LayoutPlan::new(&document.forms, &document.tokens, &anchors);
        let emitted = emit(&document, &layout)?;
        let settled = round > 0 && emitted.text == output;
        output = emitted.text;
        anchors = emitted.anchors;
        // Planning from column zero can only under-break, never over-break, so
        // a first round whose every line fits is already the minimal layout and
        // needs no second pass. Only source that overflows pays for more.
        if settled || fits_line_width(&output, &emitted.foreign) {
            break;
        }
    }

    let formatted = reader::read(&output);
    let equivalent = formatted.diagnostics.is_empty()
        && documents_equivalent_after_embedded_formatting(&document.forms, &formatted.forms);
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

/// Whether every line this formatter owns already fits the canonical width.
///
/// An embedded block carries a foreign language's own canonical layout at that
/// language's own width — Ruff's default is 88 columns — so counting its lines
/// would make the layout loop chase a target it can never reach.
fn fits_line_width(text: &str, foreign: &[std::ops::Range<usize>]) -> bool {
    let mut offset = 0;
    for line in text.split('\n') {
        let range = offset..offset + line.len();
        offset = range.end + 1;
        let embedded = foreign
            .iter()
            .any(|block| block.start < range.end && range.start < block.end);
        if !embedded && crate::text::canonical_width(line) > MAX_LINE_WIDTH {
            return false;
        }
    }
    true
}

fn emit(document: &Document, layout: &LayoutPlan) -> Result<Emitted, FormatError> {
    let mut anchors = BTreeMap::new();
    let mut foreign = Vec::new();
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
                    && !token.text.trim_start().starts_with(";; =>")
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
        let rendered = if token.kind == TokenKind::EmbeddedLanguage {
            let Some(form) = find_embedded_form(&document.forms, token.span.start) else {
                return Err(format_error(
                    "formatter could not associate an embedded-language token with its form",
                    token.span,
                ));
            };
            render_embedded(form, token_column)?
        } else {
            token.text.clone()
        };
        let rendered_start = output.len();
        output.push_str(&rendered);
        if token.kind == TokenKind::EmbeddedLanguage {
            foreign.push(rendered_start..output.len());
        }
        if let Some(last_line) = rendered.rsplit('\n').next()
            && rendered.contains('\n')
        {
            column = crate::text::canonical_width(last_line);
        } else {
            column += crate::text::canonical_width(&rendered);
        }
        line_start = false;
        if is_opening(token.kind) {
            depth += 1;
            delimiters.push((token.span.start, token_column));
            anchors.insert(token.span.start, token_column);
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
    Ok(Emitted {
        text: output,
        anchors,
        foreign,
    })
}

fn find_embedded_form(forms: &[Form], start: usize) -> Option<&Form> {
    forms.iter().find_map(|form| {
        if form.datum_span.start == start && matches!(form.kind, FormKind::EmbeddedLanguage { .. })
        {
            return Some(form);
        }
        let metadata = form.metadata.iter().find_map(|entry| {
            find_embedded_form(std::slice::from_ref(&entry.key), start)
                .or_else(|| find_embedded_form(std::slice::from_ref(&entry.value), start))
        });
        metadata.or_else(|| match &form.kind {
            FormKind::List(items)
            | FormKind::Vector(items)
            | FormKind::Map(items)
            | FormKind::Set(items) => find_embedded_form(items, start),
            FormKind::ReaderMacro { form, .. } => {
                find_embedded_form(std::slice::from_ref(form), start)
            }
            _ => None,
        })
    })
}

fn render_embedded(form: &Form, indent: usize) -> Result<String, FormatError> {
    let FormKind::EmbeddedLanguage {
        language,
        label,
        raw_body,
        body,
        body_span,
        ..
    } = &form.kind
    else {
        unreachable!("embedded rendering requires an embedded form");
    };
    let formatted_body = match language.as_str() {
        "osiris" => format_source(body).map_err(|error| FormatError {
            diagnostics: error
                .diagnostics
                .into_iter()
                .map(|mut diagnostic| {
                    diagnostic.span = crate::source::Span::new(
                        reader::embedded_source_offset(
                            raw_body,
                            body,
                            *body_span,
                            diagnostic.span.start,
                        ),
                        reader::embedded_source_offset(
                            raw_body,
                            body,
                            *body_span,
                            diagnostic.span.end,
                        ),
                    );
                    diagnostic
                })
                .collect(),
        })?,
        "python" => crate::backend::format_embedded_module(body)
            .map_err(|error| format_error(error.message(), *body_span))?,
        _ => body.clone(),
    };
    let body = formatted_body.trim_end_matches(['\r', '\n']);
    let indentation = " ".repeat(indent);
    let mut rendered = format!("~{language}<{}>\n", label.spelling);
    for line in body.split('\n') {
        rendered.push_str(&indentation);
        rendered.push_str(line.strip_suffix('\r').unwrap_or(line));
        rendered.push('\n');
    }
    rendered.push_str(&indentation);
    rendered.push_str("</");
    rendered.push_str(&label.spelling);
    rendered.push('>');
    Ok(rendered)
}

fn documents_equivalent_after_embedded_formatting(left: &[Form], right: &[Form]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    canonicalize_embedded_forms(&mut left)
        && canonicalize_embedded_forms(&mut right)
        && left
            .iter()
            .zip(&right)
            .all(|(left, right)| source_form_eq(left, right))
}

fn canonicalize_embedded_forms(forms: &mut [Form]) -> bool {
    forms.iter_mut().all(canonicalize_embedded_form)
}

fn canonicalize_embedded_form(form: &mut Form) -> bool {
    for entry in &mut form.metadata {
        if !canonicalize_embedded_form(&mut entry.key)
            || !canonicalize_embedded_form(&mut entry.value)
        {
            return false;
        }
    }
    match &mut form.kind {
        FormKind::EmbeddedLanguage {
            language,
            raw_body,
            body,
            ..
        } => {
            let canonical = match language.as_str() {
                "osiris" => match format_source(body) {
                    Ok(source) => source,
                    Err(_) => return false,
                },
                "python" => match crate::backend::format_embedded_module(body) {
                    Ok(source) => source,
                    Err(_) => return false,
                },
                _ => body.clone(),
            };
            *raw_body = canonical.clone();
            *body = canonical;
            true
        }
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => canonicalize_embedded_forms(items),
        FormKind::ReaderMacro { form, .. } => canonicalize_embedded_form(form),
        _ => true,
    }
}

fn format_error(message: impl Into<String>, span: crate::source::Span) -> FormatError {
    FormatError {
        diagnostics: vec![Diagnostic::error("OSR-F0002", message, span)],
    }
}

struct LayoutPlan<'anchors> {
    breaks_before: BTreeMap<usize, BreakSpec>,
    /// Columns measured by the previous emit round, empty on the first round.
    anchors: &'anchors BTreeMap<usize, usize>,
}

impl<'anchors> LayoutPlan<'anchors> {
    fn new(forms: &[Form], tokens: &[Token], anchors: &'anchors BTreeMap<usize, usize>) -> Self {
        let mut plan = Self {
            breaks_before: BTreeMap::new(),
            anchors,
        };
        for form in forms {
            plan.visit(form, tokens);
        }
        plan
    }

    /// Column this form's opening delimiter occupied last round.
    ///
    /// Zero on the first round, which reproduces the original plan; the second
    /// round is the one that sees where the text really sits.
    fn anchor_column(&self, form: &Form) -> usize {
        self.anchors
            .get(&form.datum_span.start)
            .copied()
            .unwrap_or(0)
    }

    /// Whether the form, laid out flat, would pass `budget` from the column it
    /// actually starts at.
    fn overflows(&self, form: &Form, tokens: &[Token], budget: usize) -> bool {
        self.anchor_column(form)
            .saturating_add(flat_form_width(form, tokens))
            > budget
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
        if items.len() > 4 || self.overflows(form, tokens, METADATA_LINE_WIDTH) {
            for key in items.iter().step_by(2).skip(1) {
                self.add_break(key.span.start, form.datum_span.start, 1);
            }
        }
    }

    fn plan_list(&mut self, form: &Form, items: &[Form], tokens: &[Token]) {
        let Some(head) = items.first().and_then(form_symbol) else {
            return;
        };
        let overflows = self.overflows(form, tokens, MAX_LINE_WIDTH);
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
            // The Clojure style guide indents the body of every `def` form and
            // of every form that takes a body by two spaces, rather than
            // aligning it under the first argument. These keep their body
            // inline while it fits, unlike `defn`, whose body always starts on
            // its own line.
            "def" | "fn" | "static-record" => {
                if overflows {
                    for body in items.iter().skip(2) {
                        self.add_break(body.span.start, form.datum_span.start, 2);
                    }
                }
            }
            "defstatic-schema" => {
                if overflows {
                    self.plan_pairs(form, items, 2);
                }
            }
            "defn" | "defn-" | "defmacro" | "defn-for-syntax" | "defstruct" => {
                if let Some(parameters) = items.get(2)
                    && self.anchor_column(form)
                        + flat_width(form.datum_span.start, parameters.span.end, tokens)
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
                let offset = crate::text::canonical_width(head) + 2;
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
            // A named body form — `(macro name (clause …) …)`, a symbol then
            // only lists — reads like `defn` and formats like it whenever it
            // breaks: name on the head line, every clause indented two. One
            // rule for the shape at every nesting depth; mixing alignment
            // into it made sibling clauses sit at two different indents.
            _ if overflows
                && items.len() > 2
                && items.get(1).and_then(form_symbol).is_some()
                && items
                    .iter()
                    .skip(2)
                    .all(|item| matches!(item.kind, FormKind::List(_))) =>
            {
                // The three-element clause `(verb name expression)` breaks as
                // late and as deep as it can: the expression's opening stays
                // on the head line and the expression folds inside itself on
                // a later layout round. Only an opening that itself cannot
                // fit hangs the whole expression.
                if items.len() == 3 {
                    let expression = &items[2];
                    // Breaking deep only helps when the expression has
                    // structure to fold; atoms would just stack one per line.
                    let foldable = match &expression.kind {
                        FormKind::List(inner) => inner.iter().skip(1).any(|item| {
                            matches!(item.kind, FormKind::List(_) | FormKind::Vector(_))
                        }),
                        _ => false,
                    };
                    let opening_end = match &expression.kind {
                        FormKind::List(inner) => inner
                            .first()
                            .map_or(expression.span.end, |head| head.span.end),
                        _ => expression.span.end,
                    };
                    let column = self.anchor_column(form);
                    let opening = flat_width(form.datum_span.start, opening_end, tokens);
                    if foldable && column + opening <= MAX_LINE_WIDTH {
                        return;
                    }
                    self.add_break(expression.span.start, form.datum_span.start, 2);
                    return;
                }
                for clause in items.iter().skip(2) {
                    self.add_break(clause.span.start, form.datum_span.start, 2);
                }
            }
            // Any other call gets the Clojure style guide's two shapes: keep
            // the first argument on the head line and align the rest under
            // it, or put no argument on the head line and indent one space.
            // Alignment reads better and is preferred, but it costs the
            // head's own width in indentation on every following line. A wide
            // head — which in CJK source is any four-character name — can
            // make that unaffordable, and the one-space shape is the guide's
            // own answer.
            _ if overflows => {
                let aligned = crate::text::canonical_width(head) + 2;
                let widest = items
                    .iter()
                    .skip(1)
                    .map(|argument| flat_width(argument.span.start, argument.span.end, tokens))
                    .max()
                    .unwrap_or(0);
                let column = self.anchor_column(form);
                if column + aligned + widest > MAX_LINE_WIDTH
                    && column + 1 + widest <= MAX_LINE_WIDTH
                {
                    for argument in items.iter().skip(1) {
                        self.add_break(argument.span.start, form.datum_span.start, 1);
                    }
                } else {
                    for argument in items.iter().skip(2) {
                        self.add_break(argument.span.start, form.datum_span.start, aligned);
                    }
                }
            }
            _ => {}
        }
    }

    fn plan_binding_vector(&mut self, form: &Form, tokens: &[Token]) {
        let FormKind::Vector(items) = &form.kind else {
            return;
        };
        if items.len() > 2 || self.overflows(form, tokens, MAX_LINE_WIDTH / 2) {
            for binding in items.iter().step_by(2).skip(1) {
                self.add_break(binding.span.start, form.datum_span.start, 1);
            }
        }
    }

    fn plan_sequential_collection(&mut self, form: &Form, items: &[Form], tokens: &[Token]) {
        if items.len() < 2 || !self.overflows(form, tokens, MAX_LINE_WIDTH) {
            return;
        }
        // Items continue one column inside the opening delimiter, so that is
        // where every packed line starts and what the budget is measured from.
        let indent = self.anchor_column(form).saturating_add(1);
        let mut line_width = indent;
        for (index, item) in items.iter().enumerate() {
            let item_width = flat_width(item.span.start, item.span.end, tokens);
            let separator = usize::from(index > 0);
            if index > 0
                && line_width
                    .saturating_add(separator)
                    .saturating_add(item_width)
                    > MAX_LINE_WIDTH
            {
                self.add_break(item.span.start, form.datum_span.start, 1);
                line_width = indent.saturating_add(item_width);
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
        width += crate::text::canonical_width(&token.text);
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
