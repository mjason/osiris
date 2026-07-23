use super::*;

pub(super) fn read_source(path: &str) -> io::Result<(String, crate::syntax::Document)> {
    let source = fs::read_to_string(path)?;
    let document = reader::read(&source);
    Ok((source, document))
}

pub(super) fn io_error(path: &str, error: &io::Error) -> CliOutcome {
    CliOutcome::failure(
        1,
        String::new(),
        format!("osr: could not read '{path}': {error}\n"),
    )
}
