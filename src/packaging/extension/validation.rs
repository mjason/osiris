fn validate_extension_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 128
        || id.chars().any(char::is_whitespace)
        || id.chars().any(char::is_control)
    {
        Err(format!("invalid extension id `{id}`"))
    } else {
        Ok(())
    }
}

fn validate_sha256(value: &str) -> Result<(), String> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err("hash must use the `sha256:` prefix".to_owned());
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("hash must contain 64 hexadecimal digits".to_owned());
    }
    Ok(())
}

#[must_use]
pub fn normalize_distribution_name(name: &str) -> String {
    let mut normalized = String::new();
    let mut separator = false;
    for character in name.chars() {
        if matches!(character, '-' | '_' | '.') {
            separator = true;
        } else {
            if separator && !normalized.is_empty() {
                normalized.push('-');
            }
            normalized.extend(character.to_lowercase());
            separator = false;
        }
    }
    normalized
}

#[must_use]
pub fn is_valid_distribution_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
