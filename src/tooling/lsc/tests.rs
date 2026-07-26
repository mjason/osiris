use super::*;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    app: PathBuf,
    app_source: String,
}

impl Fixture {
    fn create() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "osiris-lsc-service-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        let source_root = root.join("src/demo");
        fs::create_dir_all(&source_root).expect("source root");
        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname = \"lsc-service\"\nversion = \"0\"\n",
        )
        .expect("pyproject");
        fs::write(
            root.join("osiris.jsonc"),
            r#"{"source":["src"],"exclude":["src/excluded"]}"#,
        )
        .expect("configuration");
        fs::write(
            source_root.join("text.osr"),
            r#"(module demo.text)

~python<private-helper>
PRIVATE_IMPLEMENTATION_MARKER = "not graph knowledge"
</private-helper>

(export [format-message])

^{:doc
  {:default "Format a message for display."
   "zh-CN" "格式化用于显示的消息。"}
  :examples
  [["(format-message \"hello\")"]]
  :osiris/names
  {"zh-CN" {:preferred 格式化消息}}}
(defn ^Str format-message [^Str value]
  value)
"#,
        )
        .expect("provider");
        fs::write(
            source_root.join("other.osr"),
            "(module demo.other)\n\n(export [format-message])\n\n(defn ^Str format-message [^Str value]\n  value)\n",
        )
        .expect("ambiguous provider");
        let app_source = r#"(module demo.app)

(import demo.text :as text)

(def rendered (text/format-message "hello"))
"#
        .to_owned();
        let app = source_root.join("app.osr");
        fs::write(&app, &app_source).expect("consumer");
        Self {
            root,
            app,
            app_source,
        }
    }

    fn app_position(&self, needle: &str) -> SourcePosition {
        let offset = self.app_source.find(needle).expect("needle");
        let position = offset_to_position(&self.app_source, offset);
        SourcePosition {
            path: self.app.clone(),
            line: position.line + 1,
            column: position.character + 1,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn conceptual_search_uses_rich_documentation_and_localized_names() {
    let fixture = Fixture::create();
    let mut service = WorkspaceService::open(&fixture.root, Some("zh-CN")).expect("service");

    let by_doc = service.workspace_search("message for display", None);
    assert_eq!(by_doc.status, "ok", "{by_doc:?}");
    assert_eq!(
        by_doc.result[0]["data"]["bindingId"],
        "demo.text::function::format-message"
    );
    assert!(
        by_doc.result[0]["data"]["matchReasons"]
            .as_array()
            .expect("reasons")
            .iter()
            .any(|reason| reason == "semantic-graph-full-text")
    );
    assert_eq!(by_doc.result[0]["data"]["cache"], "libsql");

    let localized = service.workspace_search("格式化消息", None);
    assert_eq!(localized.status, "ok", "{localized:?}");
    assert_eq!(
        localized.result[0]["data"]["bindingId"],
        "demo.text::function::format-message"
    );
    assert_eq!(localized.result[0]["data"]["matchReasons"][0], "alias-of");
    let python_implementation = service.workspace_search("PRIVATE_IMPLEMENTATION_MARKER", None);
    assert_eq!(
        python_implementation.status, "notFound",
        "{python_implementation:?}"
    );
    assert!(
        fixture
            .root
            .join(".osiris/cache/language-graph.sqlite3")
            .is_file()
    );
}

#[test]
fn fresh_semantic_graph_opens_without_starting_the_language_server() {
    let fixture = Fixture::create();
    let first = WorkspaceService::open(&fixture.root, Some("en")).expect("initial service");
    assert!(first.machine.is_some(), "a missing cache must be analyzed");
    drop(first);

    let mut cached = WorkspaceService::open(&fixture.root, Some("en")).expect("cached service");
    assert!(
        cached.machine.is_none(),
        "a fresh graph must be opened before LSP analysis"
    );
    assert_eq!(
        cached.workspace_search("message for display", None).status,
        "ok"
    );
    assert!(
        cached.machine.is_none(),
        "graph search must not initialize the language server"
    );

    let context = cached.symbol_context("demo.text::function::format-message");
    assert_eq!(context.status, "ok", "{context:?}");
    assert!(
        cached.machine.is_some(),
        "LSP-backed context should initialize the language server lazily"
    );
}

#[test]
fn unchanged_inputs_reuse_the_persisted_file_manifest() {
    let fixture = Fixture::create();
    let project = ProjectConfig::discover(&fixture.root).expect("project");
    let initial = inputs::fingerprint(&project, None).expect("initial inputs");
    let cached = initial
        .entries
        .iter()
        .map(|entry| {
            (
                entry.identity.clone(),
                inputs::CachedInput {
                    size: entry.size,
                    stamp: entry.stamp.clone(),
                    content_hash: "cached-without-reading-source".to_owned(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    let reused = inputs::fingerprint(&project, Some(&cached)).expect("cached inputs");
    assert_eq!(reused.entries.len(), initial.entries.len());
    assert_eq!(reused.reused_hashes(), reused.entries.len());
    assert_eq!(reused.hashed_inputs(), 0);
    assert!(
        reused
            .entries
            .iter()
            .all(|entry| entry.content_hash == "cached-without-reading-source"),
        "matching size and mtime should reuse the manifest hash"
    );
}

#[test]
fn large_input_manifests_validate_without_rereading_sources() {
    const SOURCE_COUNT: usize = 4_096;

    let fixture = Fixture::create();
    let source_root = fixture.root.join("src/scale");
    fs::create_dir_all(&source_root).expect("scale source root");
    for index in 0..SOURCE_COUNT {
        fs::write(
            source_root.join(format!("m{index:04}.osr")),
            format!("(module scale.m{index:04})\n"),
        )
        .expect("scale source");
    }
    let project = ProjectConfig::discover(&fixture.root).expect("project");
    let initial = inputs::fingerprint(&project, None).expect("initial inputs");
    let cached = initial
        .entries
        .iter()
        .map(|entry| {
            (
                entry.identity.clone(),
                inputs::CachedInput {
                    size: entry.size,
                    stamp: entry.stamp.clone(),
                    content_hash: entry.content_hash.clone(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    let started = Instant::now();
    let reused = inputs::fingerprint(&project, Some(&cached)).expect("cached inputs");
    let elapsed = started.elapsed();
    assert!(reused.entries.len() >= SOURCE_COUNT);
    assert_eq!(reused.hashed_inputs(), 0);
    assert_eq!(reused.reused_hashes(), reused.entries.len());
    eprintln!(
        "validated {} cached inputs in {elapsed:?}",
        reused.entries.len()
    );
}

#[test]
fn cache_status_is_read_only_and_manual_rebuild_is_explicit() {
    let fixture = Fixture::create();
    let missing = WorkspaceService::cache_status(&fixture.root).expect("missing status");
    assert_eq!(missing.status, "missing");
    assert!(!fixture.root.join(&missing.path).exists());

    let rebuilt = WorkspaceService::rebuild_cache(&fixture.root, Some("en")).expect("rebuild");
    assert_eq!(rebuilt.status, "rebuilt");
    assert_eq!(rebuilt.input_count, rebuilt.hashed_inputs);
    assert_eq!(rebuilt.reused_hashes, 0);
    assert!(fixture.root.join(&rebuilt.path).is_file());
    let fresh = WorkspaceService::cache_status(&fixture.root).expect("fresh status");
    assert_eq!(fresh.status, "fresh");
    assert_eq!(fresh.input_count, fresh.reused_hashes);
    assert_eq!(fresh.hashed_inputs, 0);
}

#[test]
fn semantic_graph_cache_refreshes_when_compiler_facts_change() {
    let fixture = Fixture::create();
    let mut service = WorkspaceService::open(&fixture.root, Some("en")).expect("service");
    assert_eq!(
        service.workspace_search("message for display", None).status,
        "ok"
    );
    drop(service);

    let provider = fixture.root.join("src/demo/text.osr");
    let source = fs::read_to_string(&provider).expect("provider");
    fs::write(
        &provider,
        source.replace(
            "Format a message for display.",
            "Prepare a message for delivery.",
        ),
    )
    .expect("updated provider");
    let stale = WorkspaceService::cache_status(&fixture.root).expect("stale status");
    assert_eq!(stale.status, "stale");
    assert_eq!(stale.hashed_inputs, 1);
    assert_eq!(stale.reused_hashes + stale.hashed_inputs, stale.input_count);
    let mut refreshed = WorkspaceService::open(&fixture.root, Some("en")).expect("service");
    assert!(
        refreshed.machine.is_some(),
        "changed source input must trigger automatic analysis"
    );
    assert_eq!(
        refreshed
            .workspace_search("message for delivery", None)
            .status,
        "ok"
    );
}

#[test]
fn corrupted_semantic_graph_cache_is_a_rebuildable_miss() {
    let fixture = Fixture::create();
    let service = WorkspaceService::open(&fixture.root, Some("en")).expect("service");
    drop(service);
    let cache = fixture.root.join(".osiris/cache/language-graph.sqlite3");
    fs::write(&cache, b"not a database").expect("corrupted cache");

    let status = WorkspaceService::cache_status(&fixture.root).expect("invalid status");
    assert_eq!(status.status, "invalid");
    assert_eq!(
        fs::read(&cache).expect("unchanged cache"),
        b"not a database"
    );

    let mut rebuilt = WorkspaceService::open(&fixture.root, Some("en")).expect("rebuilt service");
    assert_eq!(
        rebuilt.workspace_search("message for display", None).status,
        "ok"
    );
}

#[test]
fn position_context_crosses_files_and_returns_bounded_source_facts() {
    let fixture = Fixture::create();
    let mut service = WorkspaceService::open(&fixture.root, Some("en")).expect("service");
    let result = service.position_context(&fixture.app_position("text/format-message"));

    assert_eq!(result.status, "ok", "{result:?}");
    let context = &result.result["context"];
    assert!(context["hover"]["value"].is_object(), "{context:?}");
    assert!(
        context["definition"]["value"]["uri"]
            .as_str()
            .is_some_and(|uri| uri.ends_with("/text.osr")),
        "{context:?}"
    );
    assert!(
        context["definitionSource"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("defn ^Str format-message")),
        "{context:?}"
    );
    assert!(
        context["references"]["value"]
            .as_array()
            .is_some_and(|references| references.iter().any(|reference| {
                reference["uri"]
                    .as_str()
                    .is_some_and(|uri| uri.ends_with("/app.osr"))
            })),
        "{context:?}"
    );
}

#[test]
fn equal_symbol_matches_are_reported_as_ambiguous() {
    let fixture = Fixture::create();
    let mut service = WorkspaceService::open(&fixture.root, Some("en")).expect("service");
    let result = service.symbol_context("format-message");

    assert_eq!(result.status, "ambiguous", "{result:?}");
    assert_eq!(
        result.result["candidates"].as_array().map(Vec::len),
        Some(2)
    );
}

#[test]
fn source_context_refuses_files_outside_configured_scope() {
    let fixture = Fixture::create();
    let service = WorkspaceService::open(&fixture.root, Some("en")).expect("service");
    let uri = path_to_uri(&fixture.root.join("osiris.jsonc")).expect("URI");
    let result = service.source_context(&uri, Range::default());
    assert_eq!(result.status, "unavailable", "{result:?}");
}
