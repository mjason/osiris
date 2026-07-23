fn parse_metadata(input: TokenInput<'_>, depth: usize, eof_offset: usize) -> ParseResult<'_> {
    let (mut rest, first_caret) = exact_token(input, TokenKind::Metadata)?;
    let start = first_caret.span.start;
    let mut current_caret = first_caret;
    let mut last_end = current_caret.span.end;
    let mut layers = Vec::new();
    let mut authored_entries = 0_usize;
    let mut resource_exceeded = false;
    let mut diagnostics = Vec::new();

    loop {
        let Some(next) = rest.first().copied() else {
            diagnostics.push(Diagnostic::error(
                "OSR-R0004",
                "metadata prefix `^` requires a descriptor and target",
                current_caret.span,
            ));
            return Ok((
                rest,
                ParsedForm {
                    form: Form::new(
                        FormKind::Error("missing metadata descriptor".to_owned()),
                        Span::new(start, last_end),
                    ),
                    diagnostics,
                },
            ));
        };
        if is_closing(next.kind) || next.kind == TokenKind::Metadata {
            diagnostics.push(Diagnostic::error(
                "OSR-R0004",
                "metadata prefix `^` requires a descriptor",
                current_caret.span,
            ));
            return Ok((
                rest,
                ParsedForm {
                    form: Form::new(
                        FormKind::Error("missing metadata descriptor".to_owned()),
                        Span::new(start, last_end),
                    ),
                    diagnostics,
                },
            ));
        }

        let (next_rest, descriptor) = parse_form(rest, depth + 1, eof_offset)?;
        last_end = descriptor.form.span.end;
        diagnostics.extend(descriptor.diagnostics);
        let descriptor_entries = match &descriptor.form.kind {
            FormKind::Map(items) if items.len() / 2 > METADATA_TARGET_LIMITS.max_entries => {
                resource_exceeded = true;
                diagnostics.push(Diagnostic::error(
                    "OSR-R0014",
                    format!(
                        "metadata for one syntax target exceeds the entry count limit of {} (found {})",
                        METADATA_TARGET_LIMITS.max_entries,
                        items.len() / 2
                    ),
                    descriptor.form.span,
                ));
                Vec::new()
            }
            _ => normalize_metadata_descriptor(&descriptor.form, &mut diagnostics),
        };
        let next_entry_count = authored_entries.saturating_add(descriptor_entries.len());
        if next_entry_count > METADATA_TARGET_LIMITS.max_entries {
            if !resource_exceeded {
                diagnostics.push(Diagnostic::error(
                    "OSR-R0014",
                    format!(
                        "metadata for one syntax target exceeds the entry count limit of {} (found {})",
                        METADATA_TARGET_LIMITS.max_entries, next_entry_count
                    ),
                    Span::new(start, descriptor.form.span.end),
                ));
            }
            resource_exceeded = true;
        } else if !resource_exceeded {
            authored_entries = next_entry_count;
            layers.push(descriptor_entries);
        }
        rest = next_rest;

        if rest
            .first()
            .is_some_and(|token| token.kind == TokenKind::Metadata)
        {
            current_caret = rest[0];
            last_end = current_caret.span.end;
            rest = &rest[1..];
        } else {
            break;
        }
    }

    let Some(next) = rest.first().copied() else {
        diagnostics.push(Diagnostic::error(
            "OSR-R0004",
            "metadata descriptor requires a target form",
            Span::empty(last_end),
        ));
        return Ok((
            rest,
            ParsedForm {
                form: Form::new(
                    FormKind::Error("missing metadata target".to_owned()),
                    Span::new(start, last_end),
                ),
                diagnostics,
            },
        ));
    };
    if is_closing(next.kind) {
        diagnostics.push(Diagnostic::error(
            "OSR-R0004",
            "metadata descriptor requires a target form",
            Span::empty(last_end),
        ));
        return Ok((
            rest,
            ParsedForm {
                form: Form::new(
                    FormKind::Error("missing metadata target".to_owned()),
                    Span::new(start, last_end),
                ),
                diagnostics,
            },
        ));
    }

    let (rest, mut target) = parse_form(rest, depth + 1, eof_offset)?;
    if !target.form.supports_metadata() {
        diagnostics.push(Diagnostic::error(
            "OSR-R0009",
            "metadata can only be attached to a symbol or collection form",
            target.form.datum_span,
        ));
    } else {
        let metadata = merge_metadata_layers(layers);
        match (!resource_exceeded)
            .then(|| check_metadata_resources(&metadata, METADATA_TARGET_LIMITS))
        {
            None => {}
            Some(Ok(_)) => target.form.metadata = metadata,
            Some(Err(exceeded)) => diagnostics.push(Diagnostic::error(
                "OSR-R0014",
                format!(
                    "metadata for one syntax target exceeds the {} limit of {} (found {})",
                    exceeded.resource, exceeded.limit, exceeded.actual
                ),
                Span::new(start, target.form.datum_span.start),
            )),
        }
    }
    target.form.span = Span::new(start, target.form.span.end);
    diagnostics.extend(target.diagnostics);

    Ok((
        rest,
        ParsedForm {
            form: target.form,
            diagnostics,
        },
    ))
}
