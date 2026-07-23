use super::*;

#[derive(Clone, Debug)]
pub(super) struct Requirement {
    pub(super) name: String,
    pub(super) normalized_name: String,
    pub(super) specifier: Option<String>,
    pub(super) marker: Option<String>,
    pub(super) extras: Vec<String>,
}

pub(super) fn parse_requirement(value: &str) -> Result<Requirement, String> {
    let text = value.trim();
    if text.is_empty() {
        return Err("dependency requirement is empty".to_owned());
    }
    let (left, marker) = split_once_unquoted(text, ';');
    let left = left.trim();
    let marker = marker
        .map(str::trim)
        .filter(|marker| !marker.is_empty())
        .map(str::to_owned);
    if marker
        .as_deref()
        .is_some_and(|marker| marker.to_ascii_lowercase().contains("extra"))
    {
        return Err("dependency marker using `extra` is not statically resolvable".to_owned());
    }

    let bytes = left.as_bytes();
    let mut end = 0;
    while end < bytes.len()
        && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'.' | b'_' | b'-'))
    {
        end += 1;
    }
    if end == 0 {
        return Err(format!("invalid dependency requirement `{value}`"));
    }
    let name = left[..end].to_owned();
    if !extension::is_valid_distribution_name(&name) {
        return Err(format!("invalid Python distribution name `{name}`"));
    }
    let mut remainder = left[end..].trim();
    let mut extras = Vec::new();
    if remainder.starts_with('[') {
        let close = remainder
            .find(']')
            .ok_or_else(|| format!("invalid extras in `{value}`"))?;
        extras = remainder[1..close]
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect();
        extras.sort();
        extras.dedup();
        remainder = remainder[close + 1..].trim();
    }
    if remainder.starts_with('@') || remainder.contains("://") {
        return Err(format!(
            "direct URL dependency is not represented by uv lock: `{value}`"
        ));
    }
    let specifier = (!remainder.is_empty()).then(|| remainder.to_owned());
    if let Some(specifier) = &specifier {
        parse_specifier(specifier)?;
    }
    if let Some(marker) = &marker {
        marker_applies(marker, PythonVersion::PYTHON_3_9)?;
    }
    Ok(Requirement {
        name: name.clone(),
        normalized_name: normalize_name(&name),
        specifier,
        marker,
        extras,
    })
}

pub(super) fn split_once_unquoted(value: &str, delimiter: char) -> (&str, Option<&str>) {
    let mut quote = None;
    for (index, character) in value.char_indices() {
        if quote == Some(character) {
            quote = None;
        } else if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if quote.is_none() && character == delimiter {
            return (
                &value[..index],
                Some(&value[index + character.len_utf8()..]),
            );
        }
    }
    (value, None)
}

pub(super) fn parse_specifier(value: &str) -> Result<(), String> {
    // A version stored on a uv dependency edge is exact even though project
    // requirements use PEP 440 comparator syntax.
    if !value.trim_start().starts_with(['=', '!', '~', '>', '<']) {
        version_key(value)?;
        return Ok(());
    }
    for clause in value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let (operator, expected) = split_specifier_clause(clause)
            .ok_or_else(|| format!("unsupported version specifier `{value}`"))?;
        if expected.is_empty() {
            return Err(format!("version specifier has no value: `{value}`"));
        }
        if expected != "*" {
            let numeric = expected.strip_suffix(".*").unwrap_or(expected);
            version_key(numeric)?;
        }
        if expected.ends_with(".*") && !matches!(operator, "==" | "!=") {
            return Err(format!("unsupported wildcard version specifier `{value}`"));
        }
    }
    Ok(())
}

pub(super) fn satisfies_specifier(specifier: &str, version: &str) -> Result<bool, String> {
    let specifier = specifier.trim();
    if specifier.is_empty() {
        return Ok(true);
    }
    let actual = version_key(version)?;
    if !specifier.starts_with(['=', '!', '~', '>', '<']) {
        return Ok(compare_versions(&actual, &version_key(specifier)?) == std::cmp::Ordering::Equal);
    }
    for clause in specifier
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let (operator, expected_text) = split_specifier_clause(clause)
            .ok_or_else(|| format!("unsupported version specifier `{specifier}`"))?;
        if expected_text == "*" {
            continue;
        }
        if let Some(prefix) = expected_text.strip_suffix(".*") {
            let expected = version_key(prefix)?;
            let matches = actual.iter().take(expected.len()).eq(expected.iter());
            if (operator == "==" && !matches) || (operator == "!=" && matches) {
                return Ok(false);
            }
            continue;
        }
        let expected = version_key(expected_text)?;
        let compare = compare_versions(&actual, &expected);
        let applies = match operator {
            "==" | "===" => compare == std::cmp::Ordering::Equal,
            "!=" => compare != std::cmp::Ordering::Equal,
            ">=" => compare != std::cmp::Ordering::Less,
            "<=" => compare != std::cmp::Ordering::Greater,
            ">" => compare == std::cmp::Ordering::Greater,
            "<" => compare == std::cmp::Ordering::Less,
            "~=" => {
                let prefix_len = expected.len().saturating_sub(1).max(1);
                compare != std::cmp::Ordering::Less
                    && actual
                        .iter()
                        .take(prefix_len)
                        .eq(expected.iter().take(prefix_len))
            }
            _ => return Err(format!("unsupported version specifier `{specifier}`")),
        };
        if !applies {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn split_specifier_clause(value: &str) -> Option<(&str, &str)> {
    ["===", "==", "!=", "~=", ">=", "<=", ">", "<"]
        .into_iter()
        .find_map(|operator| {
            value
                .strip_prefix(operator)
                .map(|expected| (operator, expected.trim()))
        })
}

pub(super) fn version_key(value: &str) -> Result<Vec<u64>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("empty version".to_owned());
    }
    let mut result = Vec::new();
    for component in value.split('.') {
        let digits = component
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        if digits.is_empty() {
            return Err(format!("non-numeric version `{value}`"));
        }
        result.push(
            digits
                .parse::<u64>()
                .map_err(|_| format!("version overflow `{value}`"))?,
        );
    }
    Ok(result)
}

pub(super) fn compare_versions(left: &[u64], right: &[u64]) -> std::cmp::Ordering {
    let length = left.len().max(right.len());
    (0..length)
        .map(|index| {
            (
                left.get(index).copied().unwrap_or(0),
                right.get(index).copied().unwrap_or(0),
            )
        })
        .find_map(|(left, right)| (left != right).then_some(left.cmp(&right)))
        .unwrap_or(std::cmp::Ordering::Equal)
}

pub(super) fn marker_applies(marker: &str, target: PythonVersion) -> Result<bool, String> {
    let marker = strip_outer_parentheses(marker.trim())?;
    if marker.is_empty() {
        return Ok(true);
    }
    let alternatives = split_top_level(&marker, "or")?;
    if alternatives.len() > 1 {
        for alternative in alternatives {
            if marker_applies(&alternative, target)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    let conjunction = split_top_level(&marker, "and")?;
    if conjunction.len() > 1 {
        for clause in conjunction {
            if !marker_applies(&clause, target)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    let (left, operator, right) = marker_atom(&marker)?;
    let actual = match left.as_str() {
        "python_version" => format!("{}.{}", target.major, target.minor),
        "python_full_version" => format!("{}.{}.0", target.major, target.minor),
        other => return Err(format!("marker variable `{other}` is not supported")),
    };
    let right = unquote_marker_value(&right)?;
    if matches!(operator.as_str(), "in" | "not in") {
        let found = right.split_whitespace().any(|item| item == actual);
        return Ok(if operator == "in" { found } else { !found });
    }
    let left_version = version_key(&actual)?;
    let right_version = version_key(&right)?;
    let ordering = compare_versions(&left_version, &right_version);
    Ok(match operator.as_str() {
        "==" => ordering == std::cmp::Ordering::Equal,
        "!=" => ordering != std::cmp::Ordering::Equal,
        ">=" => ordering != std::cmp::Ordering::Less,
        "<=" => ordering != std::cmp::Ordering::Greater,
        ">" => ordering == std::cmp::Ordering::Greater,
        "<" => ordering == std::cmp::Ordering::Less,
        other => return Err(format!("marker operator `{other}` is not supported")),
    })
}

pub(super) fn strip_outer_parentheses(value: &str) -> Result<String, String> {
    let mut result = value.trim().to_owned();
    loop {
        if !result.starts_with('(') || !result.ends_with(')') {
            return Ok(result);
        }
        let mut depth = 0i32;
        let mut quote = None;
        let mut encloses = true;
        for (index, character) in result.char_indices() {
            if quote == Some(character) {
                quote = None;
                continue;
            }
            if quote.is_none() && matches!(character, '\'' | '"') {
                quote = Some(character);
                continue;
            }
            if quote.is_some() {
                continue;
            }
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 && index != result.len() - 1 {
                        encloses = false;
                        break;
                    }
                    if depth < 0 {
                        return Err(format!("unbalanced marker `{value}`"));
                    }
                }
                _ => {}
            }
        }
        if quote.is_some() || depth != 0 {
            return Err(format!("unbalanced marker `{value}`"));
        }
        if encloses {
            result = result[1..result.len() - 1].trim().to_owned();
        } else {
            return Ok(result);
        }
    }
}

pub(super) fn split_top_level(value: &str, operator: &str) -> Result<Vec<String>, String> {
    let needle = format!(" {operator} ");
    let mut result = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    let mut quote = None;
    let mut index = 0;
    while index < value.len() {
        let character = value[index..]
            .chars()
            .next()
            .expect("index remains on a character boundary");
        if quote == Some(character) {
            quote = None;
            index += character.len_utf8();
            continue;
        }
        if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
            index += character.len_utf8();
            continue;
        }
        if quote.is_none() {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth < 0 {
                        return Err(format!("unbalanced marker `{value}`"));
                    }
                }
                _ => {}
            }
            if depth == 0 && value[index..].starts_with(&needle) {
                result.push(value[start..index].trim().to_owned());
                index += needle.len();
                start = index;
                continue;
            }
        }
        index += character.len_utf8();
    }
    if quote.is_some() || depth != 0 {
        return Err(format!("unbalanced marker `{value}`"));
    }
    result.push(value[start..].trim().to_owned());
    Ok(result)
}

pub(super) fn marker_atom(value: &str) -> Result<(String, String, String), String> {
    let operators = [" not in ", " in ", "==", "!=", ">=", "<=", ">", "<"];
    let mut quote = None;
    let mut depth = 0i32;
    let mut index = 0;
    while index < value.len() {
        let character = value[index..]
            .chars()
            .next()
            .expect("index remains on a character boundary");
        if quote == Some(character) {
            quote = None;
            index += character.len_utf8();
            continue;
        }
        if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
            index += character.len_utf8();
            continue;
        }
        if quote.is_none() && character == '(' {
            depth += 1;
        } else if quote.is_none() && character == ')' {
            depth -= 1;
        }
        if quote.is_none() && depth == 0 {
            for operator in operators {
                if value[index..].starts_with(operator) {
                    let left = value[..index].trim().to_ascii_lowercase();
                    let right = value[index + operator.len()..].trim().to_owned();
                    if left.is_empty() || right.is_empty() {
                        return Err(format!("invalid marker atom `{value}`"));
                    }
                    return Ok((left, operator.trim().to_owned(), right));
                }
            }
        }
        index += character.len_utf8();
    }
    Err(format!("unsupported marker atom `{value}`"))
}

pub(super) fn unquote_marker_value(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.len() >= 2 {
        let first = value.as_bytes()[0] as char;
        let last = value.as_bytes()[value.len() - 1] as char;
        if matches!(first, '\'' | '"') {
            if last != first {
                return Err(format!("unterminated marker string `{value}`"));
            }
            return Ok(value[1..value.len() - 1].to_owned());
        }
    }
    Ok(value.to_owned())
}

pub(super) fn normalize_name(value: &str) -> String {
    extension::normalize_distribution_name(value)
}

pub(super) fn validate_hash(value: &str) -> Result<(), String> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(format!("`{value}` must use the `sha256:` prefix"));
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("`{value}` must contain 64 hexadecimal digits"));
    }
    Ok(())
}
