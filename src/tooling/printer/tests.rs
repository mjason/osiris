use crate::reader::read;

use super::{render_document_json, render_document_text};

#[test]
fn text_output_normalizes_metadata() {
    let document = read("^:private (def value 1)");
    assert_eq!(
        render_document_text(&document),
        "^{:private true} (def value 1)\n"
    );
}

#[test]
fn json_output_is_versionable_structured_data() {
    let document = read("[中文 :name]");
    let output = render_document_json(&document).expect("document should serialize");
    let value: serde_json::Value = serde_json::from_str(&output).expect("output should be JSON");
    assert_eq!(value["version"], 1);
    assert_eq!(value["source_len"], 14);
    assert!(value["tokens"].is_array());
    assert!(value["forms"].is_array());
}
