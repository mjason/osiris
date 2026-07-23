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
