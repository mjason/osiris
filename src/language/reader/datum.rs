use super::*;

pub(super) fn is_closing(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::RightParen | TokenKind::RightBracket | TokenKind::RightBrace
    )
}

pub(super) fn read_atom(token: &Token) -> Form {
    let kind = match token.text.as_str() {
        "none" => FormKind::None,
        "true" => FormKind::Bool(true),
        "false" => FormKind::Bool(false),
        spelling if spelling.starts_with(':') && spelling.len() > 1 => {
            FormKind::Keyword(name(spelling))
        }
        spelling => match canonical_integer(spelling) {
            Some(integer) => FormKind::Integer(integer),
            None if is_float(spelling) => {
                FormKind::Float(spelling.replace('_', "").to_ascii_lowercase())
            }
            None => FormKind::Symbol(name(spelling)),
        },
    };
    Form::new(kind, token.span)
}

pub(super) fn name(spelling: &str) -> Name {
    Name {
        spelling: spelling.to_owned(),
        canonical: spelling.nfc().collect(),
    }
}

pub(super) fn synthetic_keyword(spelling: &str, span: Span) -> Form {
    Form::new(FormKind::Keyword(name(spelling)), span)
}

pub(super) fn canonical_integer(spelling: &str) -> Option<String> {
    let cleaned = spelling.replace('_', "");
    let (sign, digits) = match cleaned.as_bytes().first() {
        Some(b'+') => ("", &cleaned[1..]),
        Some(b'-') => ("-", &cleaned[1..]),
        _ => ("", cleaned.as_str()),
    };
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || !valid_numeric_underscores(spelling)
    {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        Some("0".to_owned())
    } else {
        Some(format!("{sign}{digits}"))
    }
}

pub(super) fn is_float(spelling: &str) -> bool {
    if !spelling
        .bytes()
        .any(|byte| matches!(byte, b'.' | b'e' | b'E'))
        || !valid_numeric_underscores(spelling)
    {
        return false;
    }
    let cleaned = spelling.replace('_', "");
    cleaned
        .parse::<f64>()
        .is_ok_and(|number| number.is_finite())
}

pub(super) fn valid_numeric_underscores(spelling: &str) -> bool {
    !spelling.starts_with('_')
        && !spelling.ends_with('_')
        && !spelling.contains("__")
        && spelling.bytes().enumerate().all(|(index, byte)| {
            byte != b'_'
                || (index > 0
                    && spelling.as_bytes()[index - 1].is_ascii_digit()
                    && spelling
                        .as_bytes()
                        .get(index + 1)
                        .is_some_and(u8::is_ascii_digit))
        })
}

pub(super) fn decode_string(spelling: &str) -> Result<String, String> {
    let body = spelling
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or_else(|| "unterminated string literal".to_owned())?;
    let mut result = String::new();
    let mut characters = body.chars().peekable();

    while let Some(character) = characters.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }

        let escaped = characters
            .next()
            .ok_or_else(|| "unterminated string escape".to_owned())?;
        match escaped {
            '"' => result.push('"'),
            '\'' => result.push('\''),
            '\\' => result.push('\\'),
            '/' => result.push('/'),
            'a' => result.push('\u{0007}'),
            'b' => result.push('\u{0008}'),
            'f' => result.push('\u{000c}'),
            'n' => result.push('\n'),
            'r' => result.push('\r'),
            't' => result.push('\t'),
            'v' => result.push('\u{000b}'),
            '\n' => {}
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
            }
            'u' => result.push(decode_hex_escape(&mut characters, 4)?),
            'U' => result.push(decode_hex_escape(&mut characters, 8)?),
            'x' => result.push(decode_hex_escape(&mut characters, 2)?),
            digit @ '0'..='7' => {
                let mut digits = String::from(digit);
                while digits.len() < 3
                    && characters
                        .peek()
                        .is_some_and(|next| ('0'..='7').contains(next))
                {
                    digits.push(characters.next().expect("peeked above"));
                }
                let value = u32::from_str_radix(&digits, 8).map_err(|error| error.to_string())?;
                result.push(
                    char::from_u32(value)
                        .ok_or_else(|| "octal escape is not a Unicode scalar".to_owned())?,
                );
            }
            other => return Err(format!("unsupported string escape `\\{other}`")),
        }
    }
    Ok(result)
}

pub(super) fn decode_hex_escape(
    characters: &mut impl Iterator<Item = char>,
    width: usize,
) -> Result<char, String> {
    let digits = characters.take(width).collect::<String>();
    if digits.chars().count() != width || !digits.chars().all(|digit| digit.is_ascii_hexdigit()) {
        return Err(format!("invalid {width}-digit hexadecimal escape"));
    }
    let value = u32::from_str_radix(&digits, 16).map_err(|error| error.to_string())?;
    char::from_u32(value).ok_or_else(|| "escape is not a Unicode scalar value".to_owned())
}

pub(super) fn normalize_metadata_descriptor(
    descriptor: &Form,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<MetadataEntry> {
    let entries = match &descriptor.kind {
        FormKind::Map(items) if items.len() % 2 == 0 => items
            .chunks_exact(2)
            .map(|pair| MetadataEntry {
                key: pair[0].clone(),
                value: pair[1].clone(),
            })
            .collect(),
        FormKind::Keyword(_) => vec![MetadataEntry {
            key: descriptor.clone(),
            value: Form::new(FormKind::Bool(true), descriptor.span),
        }],
        FormKind::Symbol(_) | FormKind::String(_) => vec![MetadataEntry {
            key: synthetic_keyword(":tag", descriptor.span),
            value: descriptor.clone(),
        }],
        FormKind::Vector(_) => vec![MetadataEntry {
            key: synthetic_keyword(":param-tags", descriptor.span),
            value: descriptor.clone(),
        }],
        _ => {
            diagnostics.push(Diagnostic::error(
                "OSR-R0005",
                "metadata descriptor must be a map, keyword, symbol, string, or vector",
                descriptor.span,
            ));
            Vec::new()
        }
    };

    for entry in &entries {
        if !metadata_datum_is_serializable(&entry.key)
            || !metadata_datum_is_serializable(&entry.value)
        {
            diagnostics.push(Diagnostic::error(
                "OSR-R0011",
                "metadata must contain only serializable phase-1 data",
                descriptor.span,
            ));
            return Vec::new();
        }
    }
    entries
}

pub(super) fn merge_metadata_layers(layers: Vec<Vec<MetadataEntry>>) -> Vec<MetadataEntry> {
    let mut merged: Vec<MetadataEntry> = Vec::new();
    for layer in layers.into_iter().rev() {
        for entry in layer {
            if let Some(existing) = merged
                .iter_mut()
                .find(|existing| datum_eq(&existing.key, &entry.key))
            {
                *existing = entry;
            } else {
                merged.push(entry);
            }
        }
    }
    merged
}
