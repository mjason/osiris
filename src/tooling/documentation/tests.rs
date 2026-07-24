use super::*;

#[test]
fn syntax_and_graphql_are_served_from_the_embedded_snapshot() {
    let syntax = syntax_markdown().expect("embedded syntax");
    assert_eq!(syntax.id, "language/syntax");
    assert!(syntax.markdown.contains("# Osiris Syntax"));
    let response = execute_graphql(
        "{ documentationCapabilities { source schemaVersion publicationChannel } }",
    )
    .expect("GraphQL");
    let value: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(
        value["data"]["documentationCapabilities"]["source"],
        "embedded"
    );
    assert_eq!(
        value["data"]["documentationCapabilities"]["publicationChannel"],
        "preview"
    );
    let response =
        execute_graphql("{ document(id: \"oep/0001\") { status normative } }").expect("OEP query");
    let value: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(value["data"]["document"]["status"], "Draft");
    assert_eq!(value["data"]["document"]["normative"], false);
}

#[test]
fn search_counts_the_full_result_and_completion_includes_headings() {
    let response = execute_graphql(
        r#"{
          searchDocuments(input: {query: "Osiris", first: 1}) {
            totalCount
            nodes { id }
          }
          completeDocumentQuery(input: {prefix: "Embedded"}) {
            documentId
            matchingHeading
          }
        }"#,
    )
    .expect("GraphQL search");
    let value: serde_json::Value = serde_json::from_str(&response).unwrap();
    let search = &value["data"]["searchDocuments"];
    assert_eq!(search["nodes"].as_array().unwrap().len(), 1);
    assert!(search["totalCount"].as_i64().unwrap() > 1);
    assert!(
        value["data"]["completeDocumentQuery"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["matchingHeading"] == "Embedded Documentation")
    );
}

#[test]
fn graphql_rejects_documents_without_exactly_one_query_operation() {
    let multiple = execute_graphql("query One { documentationCapabilities { source } } query Two { documentationCapabilities { source } }")
        .expect_err("multiple operations must be rejected");
    assert!(multiple.contains("exactly one query operation"));

    let mutation = execute_graphql("mutation { unsupported }")
        .expect_err("non-query operations must be rejected");
    assert!(mutation.contains("must select a query operation"));
}
