fn parse_string(input: TokenInput<'_>) -> ParseResult<'_> {
    let (rest, token) = exact_token(input, TokenKind::String)?;
    match decode_string(&token.text) {
        Ok(value) => Ok((
            rest,
            ParsedForm::new(Form::new(FormKind::String(value), token.span)),
        )),
        Err(message) => Ok((
            rest,
            ParsedForm::error(
                Form::new(
                    FormKind::Error("invalid string literal".to_owned()),
                    token.span,
                ),
                Diagnostic::error("OSR-R0012", message, token.span),
            ),
        )),
    }
}

fn parse_embedded_language(input: TokenInput<'_>) -> ParseResult<'_> {
    let (rest, token) = exact_token(input, TokenKind::EmbeddedLanguage)?;
    let text = token.text.as_str();
    let language_end = text
        .find('<')
        .expect("embedded-language token has an opening tag");
    let language = &text[1..language_end];
    let label_end = text[language_end + 1..]
        .find('>')
        .map(|offset| language_end + 1 + offset)
        .expect("embedded-language token has a closed opening tag");
    let label = &text[language_end + 1..label_end];
    let closing = format!("</{label}>");
    let body_start_in_token = label_end + 1;
    let body_end_in_token = text.len() - closing.len();
    let raw_body = &text[body_start_in_token..body_end_in_token];
    let body_span = Span::new(
        token.span.start + body_start_in_token,
        token.span.start + body_end_in_token,
    );
    let (body, diagnostics) = normalize_embedded_body(raw_body, body_span);
    Ok((
        rest,
        ParsedForm {
            form: Form::new(
                FormKind::EmbeddedLanguage {
                    language: language.to_owned(),
                    label: name(label),
                    raw_body: raw_body.to_owned(),
                    body,
                    body_span,
                },
                token.span,
            ),
            diagnostics,
        },
    ))
}

fn normalize_embedded_body(raw: &str, span: Span) -> (String, Vec<Diagnostic>) {
    let first_break = if raw.starts_with("\r\n") {
        Some(2)
    } else if raw.starts_with('\n') || raw.starts_with('\r') {
        Some(1)
    } else {
        None
    };
    let Some(first_break) = first_break else {
        return (raw.to_owned(), Vec::new());
    };
    let Some(last_break_char) = raw.rfind(['\n', '\r']) else {
        return (raw.to_owned(), Vec::new());
    };
    let (last_break_start, last_break_end) = if raw.as_bytes()[last_break_char] == b'\n'
        && last_break_char > 0
        && raw.as_bytes()[last_break_char - 1] == b'\r'
    {
        (last_break_char - 1, last_break_char + 1)
    } else {
        (last_break_char, last_break_char + 1)
    };
    let closing_indent = &raw[last_break_end..];
    if !closing_indent.chars().all(|character| matches!(character, ' ' | '\t')) {
        return (raw.to_owned(), Vec::new());
    }
    let content = &raw[first_break..last_break_start];
    let mut normalized = String::new();
    let mut diagnostics = Vec::new();
    let mut offset = first_break;
    let mut cursor = 0;
    while cursor < content.len() {
        let relative_break = content[cursor..].find(['\n', '\r']);
        let line_end = relative_break.map_or(content.len(), |value| cursor + value);
        let ending_end = match content.as_bytes().get(line_end) {
            Some(b'\r') if content.as_bytes().get(line_end + 1) == Some(&b'\n') => line_end + 2,
            Some(b'\r' | b'\n') => line_end + 1,
            _ => line_end,
        };
        let line = &content[cursor..line_end];
        let ending = &content[line_end..ending_end];
        if line.is_empty() {
            normalized.push_str(ending);
        } else if let Some(stripped) = line.strip_prefix(closing_indent) {
            normalized.push_str(stripped);
            normalized.push_str(ending);
        } else {
            diagnostics.push(Diagnostic::error(
                "OSR-R0014",
                "embedded body line is indented less than its closing tag",
                Span::new(span.start + offset, span.start + offset + line.len()),
            ));
            normalized.push_str(line);
            normalized.push_str(ending);
        }
        offset += ending_end - cursor;
        cursor = ending_end;
    }
    (normalized, diagnostics)
}

fn parse_atom(input: TokenInput<'_>) -> ParseResult<'_> {
    let (rest, token) = exact_token(input, TokenKind::Atom)?;
    Ok((rest, ParsedForm::new(read_atom(token))))
}

fn parse_lexical_error(input: TokenInput<'_>) -> ParseResult<'_> {
    let (rest, token) = exact_token(input, TokenKind::Error)?;
    Ok((
        rest,
        ParsedForm::new(Form::new(FormKind::Error(token.text.clone()), token.span)),
    ))
}

fn parse_unexpected_closing(input: TokenInput<'_>) -> ParseResult<'_> {
    let Some(token) = input
        .first()
        .copied()
        .filter(|token| is_closing(token.kind))
    else {
        return Err(nom::Err::Error(Error::new(input, ErrorKind::Tag)));
    };
    Ok((
        &input[1..],
        ParsedForm::error(
            Form::new(
                FormKind::Error("unexpected closing delimiter".to_owned()),
                token.span,
            ),
            Diagnostic::error(
                "OSR-R0001",
                format!("unexpected closing delimiter `{}`", token.text),
                token.span,
            ),
        ),
    ))
}

fn exact_token<'source>(
    input: TokenInput<'source>,
    expected: TokenKind,
) -> IResult<TokenInput<'source>, &'source Token> {
    let Some(token) = input
        .first()
        .copied()
        .filter(|token| token.kind == expected)
    else {
        return Err(nom::Err::Error(Error::new(input, ErrorKind::Tag)));
    };
    Ok((&input[1..], token))
}

fn validate_collection(
    kind: CollectionKind,
    items: &[Form],
    span: Span,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match kind {
        CollectionKind::Map => {
            if items.len() % 2 != 0 {
                diagnostics.push(Diagnostic::error(
                    "OSR-R0006",
                    "map literal requires an even number of forms",
                    items.last().map_or(span, |item| item.span),
                ));
            }

            let entries = items.chunks_exact(2).collect::<Vec<_>>();
            for (index, entry) in entries.iter().enumerate() {
                if entries[..index]
                    .iter()
                    .any(|previous| datum_eq(&previous[0], &entry[0]))
                {
                    diagnostics.push(Diagnostic::error(
                        "OSR-R0007",
                        "duplicate map key",
                        entry[0].span,
                    ));
                }
            }
        }
        CollectionKind::Set => {
            for (index, item) in items.iter().enumerate() {
                if items[..index]
                    .iter()
                    .any(|previous| datum_eq(previous, item))
                {
                    diagnostics.push(Diagnostic::error(
                        "OSR-R0008",
                        "duplicate set item",
                        item.span,
                    ));
                }
            }
        }
        CollectionKind::List | CollectionKind::Vector => {}
    }
}

#[derive(Clone, Copy)]
enum CollectionKind {
    List,
    Vector,
    Map,
    Set,
}
