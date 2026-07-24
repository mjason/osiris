//! Embedded, offline libSQL documentation and its read-only GraphQL schema.

use std::{sync::mpsc, time::Duration};

use async_graphql::{
    EmptyMutation, EmptySubscription, ID, InputObject, Object, Schema, SimpleObject,
};
use async_graphql_parser::{parse_query, types::OperationType};
use futures::executor::block_on;
use libsql::{Connection, params};

const DATABASE_BYTES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/osiris-documentation.sqlite3"));
const SNAPSHOT_HASH: &str = env!("OSIRIS_DOC_SNAPSHOT_HASH");
const DATABASE_HASH: &str = env!("OSIRIS_DOC_DATABASE_HASH");
const DATABASE_SCHEMA_VERSION: &str = "osiris.documentation/v2";
const SCHEMA_VERSION: &str = "osiris.documentation.graphql/v1";
const SCHEMA_CONTRACT: &str = "Document.status,DocumentChunk,DocumentConnection,DocumentCompletion,DocumentationCapabilities.publicationChannel";
const MAX_RESULTS: i32 = 100;
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

mod storage;

use storage::embedded_connection;

#[derive(Clone, SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct Document {
    id: ID,
    title: String,
    collection: String,
    normative: bool,
    status: Option<String>,
    revision: i32,
    content_hash: String,
    markdown: String,
    provenance: Provenance,
    chunks: Vec<DocumentChunk>,
}

#[derive(Clone, SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct DocumentChunk {
    anchor: String,
    heading: String,
    markdown: String,
    ordinal: i32,
}

#[derive(Clone, SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct Provenance {
    source: String,
    snapshot_id: ID,
    content_hash: String,
}

#[derive(Clone, SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct DocumentEdge {
    cursor: String,
    node: Document,
    excerpt: Option<DocumentChunk>,
}

#[derive(Clone, SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Clone, SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct DocumentConnection {
    edges: Vec<DocumentEdge>,
    nodes: Vec<Document>,
    page_info: PageInfo,
    total_count: i32,
}

#[derive(InputObject)]
#[graphql(rename_fields = "camelCase")]
struct DocumentSearchInput {
    query: String,
    #[graphql(default = 10)]
    first: i32,
    after: Option<String>,
    #[graphql(default = false)]
    include_discussions: bool,
}

#[derive(InputObject)]
struct DocumentCompletionInput {
    prefix: String,
    #[graphql(default = 20)]
    limit: i32,
}

#[derive(Clone, SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct DocumentCompletion {
    document_id: ID,
    title: String,
    matching_heading: Option<String>,
    snapshot_id: ID,
}

#[derive(Clone, SimpleObject)]
#[graphql(rename_fields = "camelCase")]
struct DocumentationCapabilities {
    source: String,
    snapshot_id: ID,
    content_hash: String,
    schema_version: String,
    schema_hash: String,
    compiler_version: String,
    language_version: String,
    publication_channel: String,
    collections: Vec<String>,
    document_count: i32,
    search: bool,
    completion: bool,
    maximum_results: i32,
}

#[derive(Clone)]
struct QueryRoot {
    connection: Connection,
}

#[Object(rename_fields = "camelCase")]
impl QueryRoot {
    async fn document(&self, id: ID) -> async_graphql::Result<Option<Document>> {
        load_document(&self.connection, id.as_str())
            .await
            .map_err(graphql_error)
    }

    async fn search_documents(
        &self,
        input: DocumentSearchInput,
    ) -> async_graphql::Result<DocumentConnection> {
        search(&self.connection, input).await.map_err(graphql_error)
    }

    async fn documentation_capabilities(&self) -> async_graphql::Result<DocumentationCapabilities> {
        capabilities(&self.connection).await.map_err(graphql_error)
    }

    async fn complete_document_query(
        &self,
        input: DocumentCompletionInput,
    ) -> async_graphql::Result<Vec<DocumentCompletion>> {
        complete(&self.connection, input)
            .await
            .map_err(graphql_error)
    }
}

/// Execute one GraphQL document and return the standard GraphQL JSON response.
pub fn execute_graphql(query: &str) -> Result<String, String> {
    if query.len() > 256 * 1024 {
        return Err("GraphQL document exceeds the 256 KiB limit".to_owned());
    }
    validate_query_document(query)?;
    let query = query.to_owned();
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("osiris-doc-query".to_owned())
        .spawn(move || {
            let result = block_on(async {
                let connection = embedded_connection().await?;
                let schema =
                    Schema::build(QueryRoot { connection }, EmptyMutation, EmptySubscription)
                        .limit_depth(12)
                        .limit_complexity(500)
                        .finish();
                Ok::<_, String>(schema.execute(query).await)
            });
            let _ = send.send(result);
        })
        .map_err(|error| format!("could not start embedded documentation query: {error}"))?;
    let response = receive
        .recv_timeout(QUERY_TIMEOUT)
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => {
                "embedded documentation query exceeded the 2 second limit".to_owned()
            }
            mpsc::RecvTimeoutError::Disconnected => {
                "embedded documentation query terminated unexpectedly".to_owned()
            }
        })??;
    let mut output = serde_json::to_string(&response).map_err(|error| error.to_string())?;
    if output.len() > 8 * 1024 * 1024 {
        return Err("GraphQL response exceeds the 8 MiB limit".to_owned());
    }
    output.push('\n');
    Ok(output)
}

fn validate_query_document(query: &str) -> Result<(), String> {
    let document = parse_query(query).map_err(|error| error.to_string())?;
    let mut operations = document.operations.iter();
    let Some((_, operation)) = operations.next() else {
        return Err("GraphQL document must select exactly one query operation".to_owned());
    };
    if operations.next().is_some() {
        return Err("GraphQL document must select exactly one query operation".to_owned());
    }
    if operation.node.ty != OperationType::Query {
        return Err("GraphQL document must select a query operation".to_owned());
    }
    Ok(())
}

/// Load the complete authored syntax manual through the embedded snapshot.
pub fn syntax_markdown() -> Result<SyntaxDocument, String> {
    block_on(async {
        let connection = embedded_connection().await?;
        let document = load_document(&connection, "language/syntax")
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "embedded snapshot does not contain language/syntax".to_owned())?;
        Ok(SyntaxDocument {
            id: document.id.to_string(),
            title: document.title,
            revision: document.revision,
            content_hash: document.content_hash,
            markdown: document.markdown,
        })
    })
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyntaxDocument {
    pub id: String,
    pub title: String,
    pub revision: i32,
    pub content_hash: String,
    pub markdown: String,
}

async fn load_document(connection: &Connection, id: &str) -> libsql::Result<Option<Document>> {
    let mut rows = connection
        .query(
            "SELECT id, title, collection, normative, status, revision, content_hash, markdown, source FROM documents WHERE id = ?1",
            [id],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let id: String = row.get(0)?;
    let hash: String = row.get(6)?;
    let chunks = load_chunks(connection, &id).await?;
    Ok(Some(Document {
        id: ID(id),
        title: row.get(1)?,
        collection: row.get(2)?,
        normative: row.get::<i64>(3)? != 0,
        status: row.get(4)?,
        revision: row.get::<i64>(5)? as i32,
        content_hash: hash.clone(),
        markdown: row.get(7)?,
        provenance: Provenance {
            source: row.get(8)?,
            snapshot_id: ID(format!("sha256:{SNAPSHOT_HASH}")),
            content_hash: hash,
        },
        chunks,
    }))
}

async fn load_chunks(connection: &Connection, id: &str) -> libsql::Result<Vec<DocumentChunk>> {
    let mut rows = connection.query("SELECT anchor, heading, body, ordinal FROM chunks WHERE document_id = ?1 ORDER BY ordinal", [id]).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(DocumentChunk {
            anchor: row.get(0)?,
            heading: row.get(1)?,
            markdown: row.get(2)?,
            ordinal: row.get::<i64>(3)? as i32,
        });
    }
    Ok(result)
}

async fn search(
    connection: &Connection,
    input: DocumentSearchInput,
) -> libsql::Result<DocumentConnection> {
    let first = input.first.clamp(1, MAX_RESULTS) as usize;
    let query = input
        .query
        .split_whitespace()
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ");
    if query.is_empty() {
        return Ok(DocumentConnection {
            edges: Vec::new(),
            nodes: Vec::new(),
            page_info: PageInfo {
                has_next_page: false,
                end_cursor: None,
            },
            total_count: 0,
        });
    }
    let after = input.after.unwrap_or_default();
    let discussion = i64::from(input.include_discussions);
    let total_count = connection
        .query(
            "SELECT COUNT(DISTINCT f.document_id) FROM document_fts f JOIN documents d ON d.id = f.document_id WHERE document_fts MATCH ?1 AND (?2 = 1 OR d.normative = 1)",
            params![query.clone(), discussion],
        )
        .await?
        .next()
        .await?
        .expect("search count row")
        .get::<i64>(0)? as i32;
    let mut rows = connection.query(
        "SELECT f.document_id, f.anchor, f.heading, f.body FROM document_fts f JOIN documents d ON d.id = f.document_id WHERE document_fts MATCH ?1 AND f.document_id > ?2 AND (?3 = 1 OR d.normative = 1) ORDER BY bm25(document_fts), f.document_id, f.anchor LIMIT 501",
        params![query, after, discussion],
    ).await?;
    let mut matches = Vec::<(String, DocumentChunk)>::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        if matches.iter().any(|(existing, _)| existing == &id) {
            continue;
        }
        matches.push((
            id,
            DocumentChunk {
                anchor: row.get(1)?,
                heading: row.get(2)?,
                markdown: row.get(3)?,
                ordinal: 0,
            },
        ));
        if matches.len() > first {
            break;
        }
    }
    let has_next_page = matches.len() > first;
    matches.truncate(first);
    let mut edges = Vec::new();
    let mut nodes = Vec::new();
    for (id, excerpt) in matches {
        if let Some(document) = load_document(connection, &id).await? {
            nodes.push(document.clone());
            edges.push(DocumentEdge {
                cursor: id,
                node: document,
                excerpt: Some(excerpt),
            });
        }
    }
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());
    Ok(DocumentConnection {
        total_count,
        edges,
        nodes,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

async fn complete(
    connection: &Connection,
    input: DocumentCompletionInput,
) -> libsql::Result<Vec<DocumentCompletion>> {
    let limit = input.limit.clamp(1, MAX_RESULTS) as i64;
    let pattern = format!(
        "{}%",
        input
            .prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let mut rows = connection.query(
        "SELECT d.id, d.title, MIN(CASE WHEN c.heading LIKE ?1 ESCAPE '\\' THEN c.heading END) FROM documents d LEFT JOIN chunks c ON c.document_id = d.id WHERE d.id LIKE ?1 ESCAPE '\\' OR d.title LIKE ?1 ESCAPE '\\' OR c.heading LIKE ?1 ESCAPE '\\' GROUP BY d.id, d.title ORDER BY d.id LIMIT ?2",
        params![pattern, limit],
    ).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(DocumentCompletion {
            document_id: ID(row.get(0)?),
            title: row.get(1)?,
            matching_heading: row.get(2)?,
            snapshot_id: ID(format!("sha256:{SNAPSHOT_HASH}")),
        });
    }
    Ok(result)
}

async fn capabilities(connection: &Connection) -> libsql::Result<DocumentationCapabilities> {
    let row = connection
        .query(
            "SELECT COUNT(*), GROUP_CONCAT(DISTINCT collection) FROM documents",
            (),
        )
        .await?
        .next()
        .await?
        .expect("aggregate row");
    let mut collection_rows = connection
        .query(
            "SELECT DISTINCT collection FROM documents ORDER BY collection",
            (),
        )
        .await?;
    let mut collections = Vec::new();
    while let Some(collection) = collection_rows.next().await? {
        collections.push(collection.get(0)?);
    }
    let publication_channel = connection
        .query(
            "SELECT value FROM metadata WHERE key = 'publication_channel'",
            (),
        )
        .await?
        .next()
        .await?
        .expect("publication channel metadata row")
        .get(0)?;
    Ok(DocumentationCapabilities {
        source: "embedded".to_owned(),
        snapshot_id: ID(format!("sha256:{SNAPSHOT_HASH}")),
        content_hash: format!("sha256:{SNAPSHOT_HASH}"),
        schema_version: SCHEMA_VERSION.to_owned(),
        schema_hash: crate::hash::sha256(SCHEMA_CONTRACT.as_bytes()),
        compiler_version: crate::version().to_owned(),
        language_version: crate::LANGUAGE_VERSION.to_owned(),
        publication_channel,
        collections,
        document_count: row.get::<i64>(0)? as i32,
        search: true,
        completion: true,
        maximum_results: MAX_RESULTS,
    })
}

fn graphql_error(error: libsql::Error) -> async_graphql::Error {
    async_graphql::Error::new(format!("embedded documentation query failed: {error}"))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
