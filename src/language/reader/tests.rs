use std::collections::BTreeSet;

use super::{MAX_DEPTH, parse_form, read, read_incremental};
use crate::{
    lexer::lex,
    syntax::{
        FormKind, METADATA_TARGET_LIMITS, NodePath, NodePathSegment, SyntaxNodeKind, TokenKind,
    },
};

#[test]
fn nom_form_parser_consumes_one_form_and_leaves_the_rest() {
    let source = "(alpha beta) gamma";
    let lexed = lex(source);
    assert!(lexed.diagnostics.is_empty());
    let significant = lexed
        .tokens
        .iter()
        .filter(|token| !token.kind.is_trivia())
        .collect::<Vec<_>>();

    let (rest, parsed) = parse_form(significant.as_slice(), 0, source.len())
        .expect("nom reader parser should parse one complete form");
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].text, "gamma");
    assert!(parsed.diagnostics.is_empty());
    assert!(matches!(
        parsed.form.kind,
        FormKind::List(items) if items.len() == 2
    ));
}

#[test]
fn every_nom_production_preserves_the_following_form() {
    let cases = [
        ("(alpha) tail", "tail"),
        ("[alpha] tail", "tail"),
        ("{:alpha 1} tail", "tail"),
        ("#{alpha} tail", "tail"),
        ("'alpha tail", "tail"),
        ("`alpha tail", "tail"),
        ("~alpha tail", "tail"),
        ("~@alpha tail", "tail"),
        ("^:private alpha tail", "tail"),
        ("\"alpha\" tail", "tail"),
        ("alpha tail", "tail"),
        ("# tag", "tag"),
        (") tail", "tail"),
    ];

    for (source, expected) in cases {
        let lexed = lex(source);
        let significant = lexed
            .tokens
            .iter()
            .filter(|token| !token.kind.is_trivia())
            .collect::<Vec<_>>();

        let (rest, _) = parse_form(significant.as_slice(), 0, source.len())
            .unwrap_or_else(|error| panic!("failed to parse `{source}`: {error:?}"));
        assert_eq!(rest.len(), 1, "unexpected remainder for `{source}`");
        assert_eq!(
            rest.first().map(|token| token.text.as_str()),
            Some(expected),
            "production consumed tokens from the following form in `{source}`"
        );
    }
}

#[test]
fn nom_form_parser_reports_eof_without_consuming_input() {
    let input: &[&crate::syntax::Token] = &[];
    let result = parse_form(input, 0, 0);
    assert!(matches!(result, Err(nom::Err::Error(_))));
}

#[test]
fn reads_unicode_and_preserves_trivia() {
    let source = "; 数据\n(归一化 values lower upper)\n";
    let document = read(source);

    assert!(!document.has_errors());
    assert_eq!(
        document
            .tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect::<String>(),
        source
    );
    assert!(
        document
            .tokens
            .iter()
            .any(|token| token.kind == TokenKind::Comment)
    );
}

#[test]
fn node_identities_are_serialized_unique_and_queryable() {
    let document = read("(same same) same");
    let ids = document
        .nodes
        .iter()
        .map(|node| node.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), document.nodes.len());
    assert_ne!(
        document.node_id(&NodePath::top_level(0)),
        document.node_id(&NodePath::top_level(1)),
        "repeated forms need distinct identities"
    );
    let top_level = document
        .node_identity(&NodePath::top_level(1))
        .expect("top-level identity");
    assert!(matches!(
        document.form_for_id(top_level.id).map(|form| &form.kind),
        Some(FormKind::Symbol(name)) if name.spelling == "same"
    ));
    let nested = NodePath::top_level(0).child(NodePathSegment::CollectionItem { index: 1 });
    assert!(matches!(
        document.form_at_path(&nested).map(|form| &form.kind),
        Some(FormKind::Symbol(name)) if name.spelling == "same"
    ));
    assert!(document.node_id(&nested).is_some());
    let encoded = serde_json::to_value(&document).expect("document should serialize");
    assert_eq!(
        encoded["nodes"].as_array().map(Vec::len),
        Some(document.nodes.len())
    );
}

#[test]
fn incremental_read_preserves_ids_across_preceding_trivia_and_form_edits() {
    let original = read("(def first 1)\n(def second 2)\n");
    let trivia = read_incremental(
        "; inserted comment\n\n(def first 1)\n(def second 2)\n",
        &original,
    );
    assert_eq!(
        original.node_id(&NodePath::top_level(0)),
        trivia.node_id(&NodePath::top_level(0))
    );
    assert_eq!(
        original.node_id(&NodePath::top_level(1)),
        trivia.node_id(&NodePath::top_level(1))
    );

    let changed = read_incremental("(def first 100)\n(def second 2)\n", &original);
    assert_ne!(
        original.node_id(&NodePath::top_level(0)),
        changed.node_id(&NodePath::top_level(0)),
        "the edited enclosing form needs a new identity"
    );
    assert_eq!(
        original.node_id(&NodePath::top_level(1)),
        changed.node_id(&NodePath::top_level(1)),
        "an unchanged following form retains its identity"
    );
}

#[test]
fn incremental_read_tracks_unchanged_forms_after_an_insertion() {
    let original = read("(def retained 2)\n");
    let inserted = read_incremental("(def added 1)\n(def retained 2)\n", &original);
    assert_eq!(
        original.node_id(&NodePath::top_level(0)),
        inserted.node_id(&NodePath::top_level(1))
    );
}

#[test]
fn duplicate_and_error_nodes_keep_distinct_stable_identities() {
    let duplicate = read("same same");
    let duplicate_after_trivia = read_incremental("; note\nsame same", &duplicate);
    assert_eq!(
        duplicate.node_id(&NodePath::top_level(0)),
        duplicate_after_trivia.node_id(&NodePath::top_level(0))
    );
    assert_eq!(
        duplicate.node_id(&NodePath::top_level(1)),
        duplicate_after_trivia.node_id(&NodePath::top_level(1))
    );
    assert_ne!(
        duplicate_after_trivia.node_id(&NodePath::top_level(0)),
        duplicate_after_trivia.node_id(&NodePath::top_level(1))
    );

    let anchored_duplicates = read("anchor same same");
    let inserted_duplicate = read_incremental("same anchor same same", &anchored_duplicates);
    assert_eq!(
        anchored_duplicates.node_id(&NodePath::top_level(1)),
        inserted_duplicate.node_id(&NodePath::top_level(2)),
        "an inserted duplicate must not steal the retained node identity"
    );
    assert_eq!(
        anchored_duplicates.node_id(&NodePath::top_level(2)),
        inserted_duplicate.node_id(&NodePath::top_level(3))
    );
    assert_ne!(
        inserted_duplicate.node_id(&NodePath::top_level(0)),
        inserted_duplicate.node_id(&NodePath::top_level(2))
    );

    let broken = read("' ) tail");
    let broken_after_trivia = read_incremental("; note\n' ) tail", &broken);
    let error_ids = broken
        .nodes
        .iter()
        .filter(|node| node.kind == SyntaxNodeKind::Error)
        .map(|node| node.id)
        .collect::<Vec<_>>();
    let shifted_error_ids = broken_after_trivia
        .nodes
        .iter()
        .filter(|node| node.kind == SyntaxNodeKind::Error)
        .map(|node| node.id)
        .collect::<Vec<_>>();
    assert!(!error_ids.is_empty());
    assert_eq!(error_ids, shifted_error_ids);
}

#[test]
fn preserves_original_unicode_spelling_while_normalizing_names() {
    let source = "(e\u{301})";
    let document = read(source);
    assert!(!document.has_errors(), "{:?}", document.diagnostics);

    let FormKind::List(items) = &document.forms[0].kind else {
        panic!("expected a list form");
    };
    let FormKind::Symbol(name) = &items[0].kind else {
        panic!("expected a symbol form");
    };
    assert_eq!(name.spelling, "e\u{301}");
    assert_eq!(name.canonical, "é");
}

#[test]
fn metadata_descriptors_are_normalized_and_leftmost_wins() {
    let document = read("^:static ^:awesome ^{:static false :bar :baz} sym");
    assert!(!document.has_errors(), "{:?}", document.diagnostics);
    let metadata = &document.forms[0].metadata;

    assert_eq!(metadata.len(), 3);
    let static_entry = metadata
            .iter()
            .find(|entry| {
                matches!(&entry.key.kind, FormKind::Keyword(name) if name.canonical == ":static")
            })
            .expect("static metadata should exist");
    assert!(matches!(static_entry.value.kind, FormKind::Bool(true)));
}

#[test]
fn supports_all_five_metadata_descriptors() {
    let document = read("^{:a 1} ^:flag ^Tag ^\"doc\" ^[A B _] target");
    assert!(!document.has_errors(), "{:?}", document.diagnostics);
    assert_eq!(document.forms[0].metadata.len(), 4);
}

#[test]
fn metadata_resource_boundaries_are_accepted_and_overflow_recovers() {
    let depth_boundary = metadata_depth_source(METADATA_TARGET_LIMITS.max_depth);
    let depth_overflow = metadata_depth_source(METADATA_TARGET_LIMITS.max_depth + 1);
    let entries_boundary = metadata_entries_source(METADATA_TARGET_LIMITS.max_entries);
    let entries_overflow = metadata_entries_source(METADATA_TARGET_LIMITS.max_entries + 1);
    let nodes_boundary = metadata_nodes_source(METADATA_TARGET_LIMITS.max_nodes);
    let nodes_overflow = metadata_nodes_source(METADATA_TARGET_LIMITS.max_nodes + 1);
    let bytes_boundary = metadata_bytes_source(METADATA_TARGET_LIMITS.max_normalized_bytes);
    let bytes_overflow = metadata_bytes_source(METADATA_TARGET_LIMITS.max_normalized_bytes + 1);

    for (source, label) in [
        (depth_boundary, "nesting depth"),
        (entries_boundary, "entry count"),
        (nodes_boundary, "node count"),
        (bytes_boundary, "normalized byte size"),
    ] {
        let document = read(&source);
        assert!(
            !document.has_errors(),
            "metadata {label} boundary should be accepted: {:?}",
            document.diagnostics
        );
        assert_eq!(document.forms.len(), 2);
        assert!(!document.forms[0].metadata.is_empty());
        assert_tail_is_read(&document);
    }

    for (source, label) in [
        (depth_overflow, "nesting depth"),
        (entries_overflow, "entry count"),
        (nodes_overflow, "node count"),
        (bytes_overflow, "normalized byte size"),
    ] {
        let document = read(&source);
        let diagnostic = document
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "OSR-R0014")
            .unwrap_or_else(|| {
                panic!(
                    "metadata {label} overflow needs OSR-R0014: {:?}",
                    document.diagnostics
                )
            });
        assert!(diagnostic.message.contains(label), "{diagnostic:?}");
        assert_eq!(document.forms.len(), 2);
        assert!(document.forms[0].metadata.is_empty());
        assert_tail_is_read(&document);
    }
}

#[test]
fn metadata_limits_do_not_apply_to_ordinary_business_data() {
    let vector = std::iter::repeat_n("value", METADATA_TARGET_LIMITS.max_nodes + 1)
        .collect::<Vec<_>>()
        .join(" ");
    let large_string = "x".repeat(METADATA_TARGET_LIMITS.max_normalized_bytes + 1);
    let source = format!("[{vector}] \"{large_string}\" tail");
    let document = read(&source);

    assert!(!document.has_errors(), "{:?}", document.diagnostics);
    assert_eq!(document.forms.len(), 3);
    assert!(
        document
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "OSR-R0014")
    );
    assert!(matches!(
        &document.forms[2].kind,
        FormKind::Symbol(name) if name.canonical == "tail"
    ));
}

fn metadata_depth_source(maximum_depth: usize) -> String {
    let collection_count = maximum_depth.saturating_sub(1);
    format!(
        "^{{:x {}value{}}} target tail",
        "[".repeat(collection_count),
        "]".repeat(collection_count)
    )
}

fn metadata_entries_source(entries: usize) -> String {
    let values = (0..entries)
        .map(|index| format!(":k{index} {index}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!("^{{{values}}} target tail")
}

fn metadata_nodes_source(nodes: usize) -> String {
    let leaves = std::iter::repeat_n("x", nodes.saturating_sub(2))
        .collect::<Vec<_>>()
        .join(" ");
    format!("^{{:x [{leaves}]}} target tail")
}

fn metadata_bytes_source(bytes: usize) -> String {
    // Normalized `{:x "..."}` contributes seven bytes beyond the UTF-8
    // payload: two braces, one separator, `:x`, and two quotes.
    let payload = "x".repeat(bytes.saturating_sub(7));
    format!("^{{:x \"{payload}\"}} target tail")
}

fn assert_tail_is_read(document: &crate::syntax::Document) {
    assert!(matches!(
        &document.forms[1].kind,
        FormKind::Symbol(name) if name.canonical == "tail"
    ));
}

#[test]
fn metadata_does_not_attach_to_scalars() {
    let document = read("^:private 42");
    assert!(
        document
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-R0009")
    );
    assert!(document.forms[0].metadata.is_empty());
}

#[test]
fn recovers_at_an_outer_closing_delimiter() {
    let document = read("([)] tail");
    let codes = document
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    assert!(codes.contains(&"OSR-R0003"));
    assert!(codes.contains(&"OSR-R0001"));
    assert_eq!(document.forms.len(), 3);
    assert!(matches!(
        &document.forms[2].kind,
        FormKind::Symbol(name) if name.canonical == "tail"
    ));
}

#[test]
fn recovers_missing_prefix_operand_without_swallowing_following_forms() {
    let document = read("' ) tail");
    let codes = document
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    assert!(codes.contains(&"OSR-R0004"));
    assert!(codes.contains(&"OSR-R0001"));
    assert!(matches!(
        document.forms.last().map(|form| &form.kind),
        Some(FormKind::Symbol(name)) if name.canonical == "tail"
    ));
}

#[test]
fn recovers_an_unclosed_string_without_swallowing_the_next_line() {
    let document = read("\"unterminated\n(tail)");

    assert_eq!(document.forms.len(), 2);
    assert!(matches!(document.forms[0].kind, FormKind::Error(_)));
    assert!(matches!(
        &document.forms[1].kind,
        FormKind::List(items)
            if matches!(
                items.as_slice(),
                [crate::syntax::Form {
                    kind: FormKind::Symbol(name),
                    ..
                }] if name.canonical == "tail"
            )
    ));
}

#[test]
fn caps_nesting_with_a_recoverable_nom_form() {
    let nesting = MAX_DEPTH + 8;
    let source = format!("{}value{}", "(".repeat(nesting), ")".repeat(nesting));
    let document = read(&source);
    assert!(
        document
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "OSR-R0010"),
        "deep input should report the reader depth limit"
    );
    assert!(!document.forms.is_empty());
}

#[test]
fn diagnoses_collection_invariants() {
    let document = read("{:a 1 :a 2 :odd} #{1 1}");
    let codes = document
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    assert!(codes.contains(&"OSR-R0006"));
    assert!(codes.contains(&"OSR-R0007"));
    assert!(codes.contains(&"OSR-R0008"));
}

#[test]
fn decodes_common_string_escapes() {
    let document = read(r#""line\n\u4e2d\x41""#);
    assert!(!document.has_errors(), "{:?}", document.diagnostics);
    assert!(matches!(
        &document.forms[0].kind,
        FormKind::String(value) if value == "line\n中A"
    ));
}

#[test]
fn decodes_wide_octal_and_line_continuation_escapes() {
    let document = read(
        r#""\141\U0001f600first\
second""#,
    );
    assert!(!document.has_errors(), "{:?}", document.diagnostics);
    assert!(matches!(
        &document.forms[0].kind,
        FormKind::String(value) if value == "a😀firstsecond"
    ));
}
