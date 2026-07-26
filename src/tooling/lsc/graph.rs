use std::{collections::BTreeMap, fs, path::PathBuf};

use futures::executor::block_on;
use libsql::{Builder, Connection, params};
use serde_json::{Value as JsonValue, json};

use crate::project::ProjectConfig;

use super::inputs::{self, CachedInput, InputSnapshot};

const GRAPH_SCHEMA: &str = "osiris.semantic-graph/v3";
const CACHE_FILE: &str = "language-graph.sqlite3";

pub(super) struct GraphStore {
    path: PathBuf,
}

pub(super) enum CacheProbe {
    Fresh {
        graph: GraphStore,
        inputs: InputSnapshot,
    },
    Refresh {
        inputs: InputSnapshot,
        reason: &'static str,
    },
}

impl GraphStore {
    pub(super) fn probe(project: &ProjectConfig) -> Result<CacheProbe, String> {
        let path = cache_path(project);
        if !path.is_file() {
            return Ok(CacheProbe::Refresh {
                inputs: inputs::fingerprint(project, None)?,
                reason: "missing",
            });
        }
        match block_on(cache_identity(&path)) {
            Ok((schema, cached_fingerprint, cached_inputs))
                if schema.as_deref() == Some(GRAPH_SCHEMA) =>
            {
                let inputs = inputs::fingerprint(project, Some(&cached_inputs))?;
                if cached_fingerprint.as_deref() == Some(&inputs.fingerprint) {
                    Ok(CacheProbe::Fresh {
                        graph: Self { path },
                        inputs,
                    })
                } else {
                    Ok(CacheProbe::Refresh {
                        inputs,
                        reason: "stale",
                    })
                }
            }
            Ok(_) => Ok(CacheProbe::Refresh {
                inputs: inputs::fingerprint(project, None)?,
                reason: "stale",
            }),
            Err(_) => Ok(CacheProbe::Refresh {
                inputs: inputs::fingerprint(project, None)?,
                reason: "invalid",
            }),
        }
    }

    pub(super) fn replace(
        project: &ProjectConfig,
        snapshot: &JsonValue,
        inputs: &InputSnapshot,
    ) -> Result<Self, String> {
        let directory = project.root.join(".osiris").join("cache");
        fs::create_dir_all(&directory).map_err(|error| {
            format!(
                "could not create semantic graph cache '{}': {error}",
                directory.display()
            )
        })?;
        let path = directory.join(CACHE_FILE);
        let graph_fingerprint = crate::hash::sha256(
            serde_json::to_vec(&json!({
                "schema": GRAPH_SCHEMA,
                "compiler": crate::version(),
                "language": crate::LANGUAGE_VERSION,
                "snapshot": snapshot,
            }))
            .map_err(|error| error.to_string())?
            .as_slice(),
        );
        if let Err(first_error) = block_on(replace_database(
            &path,
            snapshot,
            inputs,
            &graph_fingerprint,
        )) {
            // This is a disposable cache. A malformed or incompatible file is
            // a miss and must never make language services unavailable.
            for candidate in [
                path.clone(),
                PathBuf::from(format!("{}-wal", path.display())),
                PathBuf::from(format!("{}-shm", path.display())),
            ] {
                if candidate.is_file() {
                    fs::remove_file(&candidate).map_err(|error| {
                        format!(
                            "could not replace invalid semantic graph cache after {first_error}: {error}"
                        )
                    })?;
                }
            }
            block_on(replace_database(
                &path,
                snapshot,
                inputs,
                &graph_fingerprint,
            ))?;
        }
        Ok(Self { path })
    }

    pub(super) fn relative_path() -> &'static str {
        ".osiris/cache/language-graph.sqlite3"
    }

    pub(super) fn search(&self, query: &str, limit: usize) -> Result<Vec<JsonValue>, String> {
        block_on(search_database(&self.path, query, limit))
    }

    pub(super) fn neighborhood(
        &self,
        binding_id: &str,
        depth: usize,
        limit: usize,
    ) -> Result<Vec<JsonValue>, String> {
        block_on(load_neighborhood(
            &self.path,
            binding_id,
            depth.min(3),
            limit.min(100),
        ))
    }
}

fn cache_path(project: &ProjectConfig) -> PathBuf {
    project.root.join(".osiris").join("cache").join(CACHE_FILE)
}

async fn connection(path: &std::path::Path) -> Result<Connection, String> {
    let database = Builder::new_local(path)
        .build()
        .await
        .map_err(|error| format!("could not open semantic graph cache: {error}"))?;
    database
        .connect()
        .map_err(|error| format!("could not connect semantic graph cache: {error}"))
}

async fn cache_identity(
    path: &std::path::Path,
) -> Result<
    (
        Option<String>,
        Option<String>,
        BTreeMap<String, CachedInput>,
    ),
    String,
> {
    let connection = connection(path).await?;
    connection
        .query(
            "SELECT n.id
             FROM graph_nodes n
             LEFT JOIN graph_edges e ON e.source = n.id
             LEFT JOIN graph_node_fts f ON f.id = n.id
             LEFT JOIN graph_inputs i ON i.identity = n.id
             LIMIT 0",
            (),
        )
        .await
        .map_err(|error| format!("invalid semantic graph cache schema: {error}"))?;
    let mut rows = connection
        .query(
            "SELECT identity, size, stamp, content_hash FROM graph_inputs ORDER BY identity",
            (),
        )
        .await
        .map_err(|error| format!("could not read semantic graph inputs: {error}"))?;
    let mut inputs = BTreeMap::new();
    while let Some(row) = rows.next().await.map_err(|error| error.to_string())? {
        let identity: String = row.get(0).map_err(|error| error.to_string())?;
        let size = row
            .get::<i64>(1)
            .map_err(|error| error.to_string())?
            .try_into()
            .map_err(|_| "semantic graph input has a negative size".to_owned())?;
        inputs.insert(
            identity,
            CachedInput {
                size,
                stamp: row.get(2).map_err(|error| error.to_string())?,
                content_hash: row.get(3).map_err(|error| error.to_string())?,
            },
        );
    }
    Ok((
        metadata_value(&connection, "schema").await?,
        metadata_value(&connection, "input-fingerprint").await?,
        inputs,
    ))
}

async fn replace_database(
    path: &std::path::Path,
    snapshot: &JsonValue,
    inputs: &InputSnapshot,
    graph_fingerprint: &str,
) -> Result<(), String> {
    let connection = connection(path).await?;
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS graph_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS graph_nodes (
               id TEXT PRIMARY KEY,
               kind TEXT NOT NULL,
               name TEXT NOT NULL,
               module TEXT NOT NULL,
               uri TEXT,
               start_line INTEGER,
               start_character INTEGER,
               search_text TEXT NOT NULL,
               payload TEXT NOT NULL
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS graph_edges (
               source TEXT NOT NULL,
               target TEXT NOT NULL,
               kind TEXT NOT NULL,
               uri TEXT NOT NULL,
               start_line INTEGER NOT NULL,
               start_character INTEGER NOT NULL,
               payload TEXT NOT NULL,
               PRIMARY KEY(source, target, kind, uri, start_line, start_character)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS graph_inputs (
               identity TEXT PRIMARY KEY,
               size INTEGER NOT NULL,
               stamp TEXT NOT NULL,
               content_hash TEXT NOT NULL
             ) WITHOUT ROWID;
             CREATE INDEX IF NOT EXISTS graph_edges_target ON graph_edges(target, kind);
             CREATE VIRTUAL TABLE IF NOT EXISTS graph_node_fts USING fts5(
               id UNINDEXED, name, module, content, tokenize='unicode61'
             );",
        )
        .await
        .map_err(|error| format!("could not initialize semantic graph cache: {error}"))?;
    let transaction = connection
        .transaction()
        .await
        .map_err(|error| format!("could not update semantic graph cache: {error}"))?;
    transaction
        .execute("DELETE FROM graph_node_fts", ())
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM graph_edges", ())
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM graph_nodes", ())
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM graph_inputs", ())
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM graph_metadata", ())
        .await
        .map_err(|error| error.to_string())?;

    for node in snapshot["nodes"].as_array().into_iter().flatten() {
        let id = node["id"].as_str().unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let kind = node["kind"].as_str().unwrap_or("symbol");
        let name = node["name"].as_str().unwrap_or(id);
        let module = node["module"].as_str().unwrap_or_default();
        let uri = node.pointer("/location/uri").and_then(JsonValue::as_str);
        let line = node
            .pointer("/location/range/start/line")
            .and_then(JsonValue::as_i64);
        let character = node
            .pointer("/location/range/start/character")
            .and_then(JsonValue::as_i64);
        let search_text = graph_search_text(node);
        let payload = serde_json::to_string(node).map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO graph_nodes VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    kind,
                    name,
                    module,
                    uri,
                    line,
                    character,
                    search_text.clone(),
                    payload
                ],
            )
            .await
            .map_err(|error| format!("could not index semantic graph node `{id}`: {error}"))?;
        transaction
            .execute(
                "INSERT INTO graph_node_fts VALUES (?1, ?2, ?3, ?4)",
                params![id, name, module, search_text],
            )
            .await
            .map_err(|error| error.to_string())?;
    }
    for edge in snapshot["edges"].as_array().into_iter().flatten() {
        let source = edge["from"].as_str().unwrap_or_default();
        let target = edge["to"].as_str().unwrap_or_default();
        if source.is_empty() || target.is_empty() {
            continue;
        }
        let kind = edge["kind"].as_str().unwrap_or("references");
        let uri = edge["uri"].as_str().unwrap_or_default();
        let line = edge
            .pointer("/range/start/line")
            .and_then(JsonValue::as_i64)
            .unwrap_or_default();
        let character = edge
            .pointer("/range/start/character")
            .and_then(JsonValue::as_i64)
            .unwrap_or_default();
        let payload = serde_json::to_string(edge).map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO graph_edges VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![source, target, kind, uri, line, character, payload],
            )
            .await
            .map_err(|error| error.to_string())?;
    }
    for input in &inputs.entries {
        transaction
            .execute(
                "INSERT INTO graph_inputs VALUES (?1, ?2, ?3, ?4)",
                params![
                    input.identity.clone(),
                    i64::try_from(input.size)
                        .map_err(|_| "semantic graph input exceeds SQLite size".to_owned())?,
                    input.stamp.clone(),
                    input.content_hash.clone()
                ],
            )
            .await
            .map_err(|error| error.to_string())?;
    }
    transaction
        .execute(
            "INSERT INTO graph_metadata VALUES ('schema', ?1)",
            [GRAPH_SCHEMA],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO graph_metadata VALUES ('input-fingerprint', ?1)",
            [inputs.fingerprint.as_str()],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO graph_metadata VALUES ('graph-fingerprint', ?1)",
            [graph_fingerprint],
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| format!("could not commit semantic graph cache: {error}"))?;
    Ok(())
}

async fn metadata_value(connection: &Connection, key: &str) -> Result<Option<String>, String> {
    let mut rows = connection
        .query("SELECT value FROM graph_metadata WHERE key = ?1", [key])
        .await
        .map_err(|error| error.to_string())?;
    rows.next()
        .await
        .map_err(|error| error.to_string())?
        .map(|row| row.get(0).map_err(|error| error.to_string()))
        .transpose()
}

async fn search_database(
    path: &std::path::Path,
    query: &str,
    limit: usize,
) -> Result<Vec<JsonValue>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let connection = connection(path).await?;
    let fts_query = query
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ");
    let mut rows = connection
        .query(
            "SELECT COALESCE(target.payload, n.payload), bm25(graph_node_fts),
                    (SELECT COUNT(*) FROM graph_edges e
                     WHERE e.source = COALESCE(target.id, n.id) OR e.target = COALESCE(target.id, n.id)),
                    n.id
             FROM graph_node_fts
             JOIN graph_nodes n ON n.id = graph_node_fts.id
             LEFT JOIN graph_edges alias_edge
               ON n.kind = 'alias' AND alias_edge.source = n.id AND alias_edge.kind = 'alias-of'
             LEFT JOIN graph_nodes target ON target.id = alias_edge.target
             WHERE graph_node_fts MATCH ?1
             ORDER BY CASE WHEN n.name = ?2 OR n.id = ?2 THEN 0 ELSE 1 END,
                      bm25(graph_node_fts), n.id
             LIMIT ?3",
            params![fts_query, query, i64::try_from(limit).unwrap_or(12)],
        )
        .await
        .map_err(|error| format!("semantic graph search failed: {error}"))?;
    let mut result = Vec::<JsonValue>::new();
    while let Some(row) = rows.next().await.map_err(|error| error.to_string())? {
        let payload: String = row.get(0).map_err(|error| error.to_string())?;
        let mut node: JsonValue =
            serde_json::from_str(&payload).map_err(|error| error.to_string())?;
        let matched_node: String = row.get(3).map_err(|error| error.to_string())?;
        let binding_id = node["id"].as_str().unwrap_or_default().to_owned();
        if result
            .iter()
            .any(|existing| existing["data"]["bindingId"] == binding_id)
        {
            continue;
        }
        node["data"] = json!({
            "bindingId": node["id"],
            "score": if matched_node.starts_with("alias:") { 100 } else { graph_score(&node, query) },
            "matchReasons": if matched_node.starts_with("alias:") { json!(["alias-of"]) } else { json!(["semantic-graph-full-text"]) },
            "matchedNode": matched_node,
            "neighborCount": row.get::<i64>(2).map_err(|error| error.to_string())?,
            "cache": "libsql",
        });
        result.push(node);
    }
    Ok(result)
}

async fn load_neighborhood(
    path: &std::path::Path,
    binding_id: &str,
    depth: usize,
    limit: usize,
) -> Result<Vec<JsonValue>, String> {
    let connection = connection(path).await?;
    let mut rows = connection
        .query(
            "WITH RECURSIVE walk(id, depth) AS (
               SELECT ?1, 0
               UNION
               SELECT CASE WHEN e.source = walk.id THEN e.target ELSE e.source END, walk.depth + 1
               FROM walk JOIN graph_edges e ON e.source = walk.id OR e.target = walk.id
               WHERE walk.depth < ?2
             )
             SELECT DISTINCT e.payload
             FROM walk JOIN graph_edges e ON e.source = walk.id OR e.target = walk.id
             ORDER BY e.kind, e.source, e.target, e.uri, e.start_line, e.start_character
             LIMIT ?3",
            params![
                binding_id,
                i64::try_from(depth).unwrap_or(2),
                i64::try_from(limit).unwrap_or(40)
            ],
        )
        .await
        .map_err(|error| format!("semantic graph traversal failed: {error}"))?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await.map_err(|error| error.to_string())? {
        let payload: String = row.get(0).map_err(|error| error.to_string())?;
        result.push(serde_json::from_str(&payload).map_err(|error| error.to_string())?);
    }
    Ok(result)
}

fn graph_search_text(node: &JsonValue) -> String {
    let mut values = vec![
        node["id"].as_str().unwrap_or_default().to_owned(),
        node["name"].as_str().unwrap_or_default().to_owned(),
        node["module"].as_str().unwrap_or_default().to_owned(),
        node["type"].to_string(),
    ];
    if let Some(documentation) = node["documentation"].as_object() {
        values.extend(
            documentation
                .values()
                .flat_map(json_strings)
                .map(str::to_owned),
        );
    }
    values.extend(json_strings(&node["names"]).map(str::to_owned));
    values.extend(json_strings(&node["aliases"]).map(str::to_owned));
    values.extend(json_strings(&node["examples"]).map(str::to_owned));
    values.join("\n")
}

fn json_strings(value: &JsonValue) -> Box<dyn Iterator<Item = &str> + '_> {
    match value {
        JsonValue::String(value) => Box::new(std::iter::once(value.as_str())),
        JsonValue::Array(values) => Box::new(values.iter().flat_map(json_strings)),
        JsonValue::Object(values) => Box::new(values.values().flat_map(json_strings)),
        _ => Box::new(std::iter::empty()),
    }
}

fn graph_score(node: &JsonValue, query: &str) -> u64 {
    let query = query.to_lowercase();
    if node["id"]
        .as_str()
        .is_some_and(|value| value.to_lowercase() == query)
    {
        120
    } else if node["name"]
        .as_str()
        .is_some_and(|value| value.to_lowercase() == query)
    {
        110
    } else {
        60
    }
}
