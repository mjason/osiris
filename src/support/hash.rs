use sha2::{Digest, Sha256};

/// Encode a SHA-256 digest in the canonical form used by Osiris artifacts.
pub(crate) fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Append one unambiguous field to a canonical hash payload.
pub(crate) fn push_field(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(value.len().to_string().as_bytes());
    output.push(b':');
    output.extend_from_slice(value.as_bytes());
    output.push(b'\n');
}

#[cfg(test)]
mod tests {
    use super::sha256;

    #[test]
    fn uses_the_canonical_prefixed_lowercase_encoding() {
        assert_eq!(
            sha256(b"osiris"),
            "sha256:d14b6c9eeebdd0d6db62dccd34678ab88e04e48a55e3029f5595ecf791c0381b"
        );
    }
}
