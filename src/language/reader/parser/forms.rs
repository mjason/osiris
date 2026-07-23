use super::*;

pub(super) fn parse_form(
    input: TokenInput<'_>,
    depth: usize,
    eof_offset: usize,
) -> ParseResult<'_> {
    let Some(first) = input.first().copied() else {
        return Err(nom::Err::Error(Error::new(input, ErrorKind::Eof)));
    };
    if depth >= MAX_DEPTH {
        return Ok((
            &input[1..],
            ParsedForm::error(
                Form::new(FormKind::Error("reader depth limit".to_owned()), first.span),
                Diagnostic::error(
                    "OSR-R0010",
                    format!("reader nesting exceeds the limit of {MAX_DEPTH}"),
                    first.span,
                ),
            ),
        ));
    }

    alt((
        |input| {
            parse_collection(
                input,
                depth,
                eof_offset,
                TokenKind::LeftParen,
                TokenKind::RightParen,
                ")",
                CollectionKind::List,
            )
        },
        |input| {
            parse_collection(
                input,
                depth,
                eof_offset,
                TokenKind::LeftBracket,
                TokenKind::RightBracket,
                "]",
                CollectionKind::Vector,
            )
        },
        |input| {
            parse_collection(
                input,
                depth,
                eof_offset,
                TokenKind::LeftBrace,
                TokenKind::RightBrace,
                "}",
                CollectionKind::Map,
            )
        },
        |input| {
            parse_collection(
                input,
                depth,
                eof_offset,
                TokenKind::SetStart,
                TokenKind::RightBrace,
                "}",
                CollectionKind::Set,
            )
        },
        |input| {
            parse_prefix(
                input,
                depth,
                eof_offset,
                TokenKind::Quote,
                ReaderMacroKind::Quote,
            )
        },
        |input| {
            parse_prefix(
                input,
                depth,
                eof_offset,
                TokenKind::SyntaxQuote,
                ReaderMacroKind::SyntaxQuote,
            )
        },
        |input| {
            parse_prefix(
                input,
                depth,
                eof_offset,
                TokenKind::Unquote,
                ReaderMacroKind::Unquote,
            )
        },
        |input| {
            parse_prefix(
                input,
                depth,
                eof_offset,
                TokenKind::UnquoteSplicing,
                ReaderMacroKind::UnquoteSplicing,
            )
        },
        |input| parse_metadata(input, depth, eof_offset),
        parse_string,
        parse_atom,
        parse_lexical_error,
        parse_unexpected_closing,
    ))
    .parse(input)
}

#[allow(clippy::too_many_arguments)]
fn parse_collection<'source>(
    input: TokenInput<'source>,
    depth: usize,
    eof_offset: usize,
    opener_kind: TokenKind,
    closer_kind: TokenKind,
    closer_spelling: &'static str,
    collection_kind: CollectionKind,
) -> ParseResult<'source> {
    let (mut rest, opener) = exact_token(input, opener_kind)?;
    let mut items = Vec::new();
    let mut diagnostics = Vec::new();

    let end = loop {
        let Some(next) = rest.first().copied() else {
            diagnostics.push(Diagnostic::error(
                "OSR-R0002",
                format!("missing closing delimiter `{closer_spelling}`"),
                Span::empty(eof_offset),
            ));
            break eof_offset;
        };

        if next.kind == closer_kind {
            rest = &rest[1..];
            break next.span.end;
        }
        if is_closing(next.kind) {
            diagnostics.push(Diagnostic::error(
                "OSR-R0003",
                format!(
                    "expected closing delimiter `{closer_spelling}` before `{}`",
                    next.text
                ),
                next.span,
            ));
            break next.span.start;
        }

        match parse_form(rest, depth + 1, eof_offset) {
            Ok((next_rest, parsed)) if next_rest.len() < rest.len() => {
                rest = next_rest;
                items.push(parsed.form);
                diagnostics.extend(parsed.diagnostics);
            }
            _ => {
                diagnostics.push(Diagnostic::error(
                    "OSR-R0013",
                    "reader could not make progress inside a collection",
                    next.span,
                ));
                items.push(Form::new(
                    FormKind::Error("unreadable collection item".to_owned()),
                    next.span,
                ));
                rest = &rest[1..];
            }
        }
    };

    let span = Span::new(opener.span.start, end);
    validate_collection(collection_kind, &items, span, &mut diagnostics);
    let kind = match collection_kind {
        CollectionKind::List => FormKind::List(items),
        CollectionKind::Vector => FormKind::Vector(items),
        CollectionKind::Map => FormKind::Map(items),
        CollectionKind::Set => FormKind::Set(items),
    };
    Ok((
        rest,
        ParsedForm {
            form: Form::new(kind, span),
            diagnostics,
        },
    ))
}

fn parse_prefix<'source>(
    input: TokenInput<'source>,
    depth: usize,
    eof_offset: usize,
    prefix_kind: TokenKind,
    macro_kind: ReaderMacroKind,
) -> ParseResult<'source> {
    let (rest, prefix) = exact_token(input, prefix_kind)?;
    let Some(next) = rest.first().copied() else {
        return Ok((
            rest,
            ParsedForm::error(
                Form::new(
                    FormKind::Error("missing reader prefix operand".to_owned()),
                    prefix.span,
                ),
                Diagnostic::error(
                    "OSR-R0004",
                    format!("reader prefix `{}` requires a form", prefix.text),
                    prefix.span,
                ),
            ),
        ));
    };
    if is_closing(next.kind) {
        return Ok((
            rest,
            ParsedForm::error(
                Form::new(
                    FormKind::Error("missing reader prefix operand".to_owned()),
                    prefix.span,
                ),
                Diagnostic::error(
                    "OSR-R0004",
                    format!("reader prefix `{}` requires a form", prefix.text),
                    prefix.span,
                ),
            ),
        ));
    }

    let (rest, parsed) = parse_form(rest, depth + 1, eof_offset)?;
    let span = prefix.span.cover(parsed.form.span);
    Ok((
        rest,
        ParsedForm {
            form: Form::new(
                FormKind::ReaderMacro {
                    macro_kind,
                    form: Box::new(parsed.form),
                },
                span,
            ),
            diagnostics: parsed.diagnostics,
        },
    ))
}
