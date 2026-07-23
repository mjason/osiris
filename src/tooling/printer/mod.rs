use std::fmt::Write;

use crate::syntax::{Document, Form, FormKind, ReaderMacroKind};

/// Prints normalized, reader-compatible forms, one top-level form per line.
#[must_use]
pub fn render_document_text(document: &Document) -> String {
    let mut output = String::new();
    for form in &document.forms {
        render_form(&mut output, form);
        output.push('\n');
    }
    output
}

/// Serializes the lossless token stream, recovered forms, metadata, and diagnostics.
pub fn render_document_json(document: &Document) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(document).map(|mut output| {
        output.push('\n');
        output
    })
}

fn render_form(output: &mut String, form: &Form) {
    if !form.metadata.is_empty() {
        output.push_str("^{");
        for (index, entry) in form.metadata.iter().enumerate() {
            if index > 0 {
                output.push(' ');
            }
            render_form(output, &entry.key);
            output.push(' ');
            render_form(output, &entry.value);
        }
        output.push_str("} ");
    }

    match &form.kind {
        FormKind::None => output.push_str("none"),
        FormKind::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        FormKind::Integer(value) | FormKind::Float(value) => output.push_str(value),
        FormKind::String(value) => {
            let encoded = serde_json::to_string(value).expect("serializing a string cannot fail");
            output.push_str(&encoded);
        }
        FormKind::Keyword(name) | FormKind::Symbol(name) => output.push_str(&name.spelling),
        FormKind::List(items) => render_collection(output, "(", ")", items),
        FormKind::Vector(items) => render_collection(output, "[", "]", items),
        FormKind::Map(items) => render_collection(output, "{", "}", items),
        FormKind::Set(items) => render_collection(output, "#{", "}", items),
        FormKind::ReaderMacro { macro_kind, form } => {
            output.push_str(match macro_kind {
                ReaderMacroKind::Quote => "'",
                ReaderMacroKind::SyntaxQuote => "`",
                ReaderMacroKind::Unquote => "~",
                ReaderMacroKind::UnquoteSplicing => "~@",
            });
            render_form(output, form);
        }
        FormKind::Error(message) => {
            let _ = write!(output, "#<error:{}>", message.replace('>', "\\>"));
        }
    }
}

fn render_collection(output: &mut String, open: &str, close: &str, items: &[Form]) {
    output.push_str(open);
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        render_form(output, item);
    }
    output.push_str(close);
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
