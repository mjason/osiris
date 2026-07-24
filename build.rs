use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
};

use futures::executor::block_on;
use libsql::{Builder, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[path = "src/jsonc.rs"]
mod jsonc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: String,
    #[serde(rename = "normativeLocale")]
    normative_locale: String,
    locales: BTreeMap<String, String>,
    documents: Vec<OepEntry>,
    manuals: Vec<ManualEntry>,
    #[serde(rename = "documentationPublication")]
    publication: DocumentationPublication,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentationPublication {
    locale: String,
    artifact: PublicationArtifact,
    channels: PublicationChannels,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
struct PublicationArtifact {
    engine: String,
    format: String,
    full_text_search: String,
    embedded: bool,
    read_only: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicationChannels {
    stable: PublicationChannel,
    preview: PublicationChannel,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct PublicationChannel {
    version_kinds: Vec<String>,
    reference_statuses: Vec<String>,
    discussion_statuses: Vec<String>,
    exclude: Vec<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OepEntry {
    number: u32,
    source: String,
    #[serde(default)]
    translations: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManualEntry {
    id: String,
    source: String,
    #[serde(default)]
    translations: BTreeMap<String, String>,
}

struct Document {
    id: String,
    title: String,
    collection: String,
    normative: bool,
    status: Option<String>,
    revision: u32,
    markdown: String,
    source: String,
    hash: String,
}

fn main() {
    println!("cargo:rerun-if-changed=oeps/oeps.jsonc");
    println!("cargo:rerun-if-changed=docs/syntax.md");
    for entry in fs::read_dir("oeps").expect("read oeps") {
        let path = entry.expect("OEP entry").path();
        if path.extension().and_then(|value| value.to_str()) == Some("md") {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    let manifest_source = fs::read_to_string("oeps/oeps.jsonc").expect("read OEP manifest");
    jsonc::validate_no_duplicate_keys(&manifest_source)
        .expect("OEP manifest must not contain duplicate object keys");
    let manifest: Manifest = json5::from_str(&manifest_source).expect("parse OEP manifest");
    validate_manifest(&manifest);
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_owned());
    let package_version = env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let version_kind = if profile != "release" {
        "development"
    } else if package_version.contains('-') {
        "prerelease"
    } else {
        "final"
    };
    let (channel_name, channel) = if version_kind == "final" {
        ("stable", &manifest.publication.channels.stable)
    } else {
        ("preview", &manifest.publication.channels.preview)
    };
    assert!(
        channel
            .version_kinds
            .iter()
            .any(|candidate| candidate == version_kind),
        "documentation channel does not admit version kind `{version_kind}`"
    );
    let mut documents = Vec::new();
    for manual in manifest.manuals {
        let path = Path::new("oeps").join(&manual.source);
        println!("cargo:rerun-if-changed={}", path.display());
        documents.push(load_document(&path, manual.id, "reference", true));
    }
    for oep in manifest.documents {
        if oep.number == 0 || channel.exclude.contains(&oep.number) {
            continue;
        }
        let path = Path::new("oeps").join(&oep.source);
        let markdown = fs::read_to_string(&path).expect("read OEP source");
        let status = front_matter(&markdown, "status").unwrap_or_default();
        let reference = channel.reference_statuses.contains(&status);
        let discussion = channel.discussion_statuses.contains(&status);
        if reference || discussion {
            documents.push(document_from_source(
                path,
                format!("oep/{:04}", oep.number),
                if reference {
                    "reference"
                } else {
                    "discussions"
                },
                reference,
                markdown,
            ));
        }
    }
    documents.sort_by(|left, right| left.id.cmp(&right.id));

    let snapshot_hash = corpus_hash(&documents);
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let database = out.join("osiris-documentation.sqlite3");
    build_database(&database, &documents, &snapshot_hash, channel_name);
    let database_hash = hash_bytes(&fs::read(&database).expect("read documentation database"));
    println!("cargo:rustc-env=OSIRIS_DOC_SNAPSHOT_HASH={snapshot_hash}");
    println!("cargo:rustc-env=OSIRIS_DOC_DATABASE_HASH={database_hash}");
    let standard_root = Path::new("stdlib");
    println!("cargo:rerun-if-changed={}", standard_root.display());
    println!(
        "cargo:rustc-env=OSIRIS_STDLIB_TREE_HASH={}",
        standard_resource_hash(standard_root)
    );
}

fn standard_resource_hash(root: &Path) -> String {
    let mut files = Vec::new();
    collect_standard_resources(root, root, &mut files);
    files.sort();
    let mut digest = Sha256::new();
    for path in files {
        let relative = path
            .strip_prefix(root)
            .expect("standard resource path")
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(&path).expect("read standard resource");
        digest.update((relative.len() as u64).to_be_bytes());
        digest.update(relative.as_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    format!("sha256:{:x}", digest.finalize())
}

fn collect_standard_resources(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("read standard resource directory") {
        let path = entry.expect("standard resource entry").path();
        if path.is_dir() {
            collect_standard_resources(root, &path, files);
        } else {
            let relative = path.strip_prefix(root).expect("standard resource path");
            let named_resource = matches!(
                relative.to_string_lossy().replace('\\', "/").as_str(),
                "README.md" | "pyproject.toml" | "osiris.jsonc" | "uv.lock"
            );
            if named_resource || relative.extension().is_some_and(|value| value == "osr") {
                files.push(path);
            }
        }
    }
}

fn validate_manifest(manifest: &Manifest) {
    assert_eq!(
        manifest.schema, "osiris.documentation-manifest/v2",
        "unsupported OEP manifest schema"
    );
    assert_eq!(
        manifest.normative_locale, "en",
        "embedded documentation must use authored English sources"
    );
    assert_eq!(
        manifest.publication.locale, manifest.normative_locale,
        "documentation publication locale must match normativeLocale"
    );
    assert_eq!(
        manifest.locales.get("en").map(String::as_str),
        Some("."),
        "the normative English locale must resolve to the OEP directory"
    );
    let artifact = &manifest.publication.artifact;
    assert!(
        artifact.engine == "libsql"
            && artifact.format == "sqlite3"
            && artifact.full_text_search == "fts5"
            && artifact.embedded
            && artifact.read_only,
        "documentation artifact must be embedded read-only libSQL/SQLite with FTS5"
    );
    assert_eq!(
        manifest.publication.channels.stable.version_kinds,
        ["final"],
        "stable documentation must admit only final versions"
    );
    assert_eq!(
        manifest.publication.channels.preview.version_kinds,
        ["prerelease", "development"],
        "preview documentation must admit prerelease and development versions"
    );

    let mut numbers = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for entry in &manifest.documents {
        assert!(numbers.insert(entry.number), "duplicate OEP number");
        validate_source_path(&entry.source);
        validate_translations(&manifest.locales, &entry.translations);
        let markdown = fs::read_to_string(Path::new("oeps").join(&entry.source))
            .expect("read OEP declared by manifest");
        assert_eq!(
            front_matter(&markdown, "oep").as_deref(),
            Some(entry.number.to_string().as_str()),
            "manifest OEP number must match source front matter"
        );
        for key in ["title", "status", "revision"] {
            assert!(
                front_matter(&markdown, key).is_some(),
                "OEP source is missing `{key}` front matter"
            );
        }
    }
    assert!(numbers.contains(&0), "manifest must declare OEP-0000");
    for manual in &manifest.manuals {
        assert!(
            ids.insert(manual.id.as_str()),
            "duplicate manual document ID"
        );
        validate_source_path(&manual.source);
        validate_translations(&manifest.locales, &manual.translations);
        let markdown = fs::read_to_string(Path::new("oeps").join(&manual.source))
            .expect("read manual declared by manifest");
        assert_eq!(
            front_matter(&markdown, "document-id").as_deref(),
            Some(manual.id.as_str()),
            "manual ID must match source front matter"
        );
    }
}

fn validate_translations(
    locales: &BTreeMap<String, String>,
    translations: &BTreeMap<String, String>,
) {
    for (locale, source) in translations {
        assert!(
            locales.contains_key(locale),
            "translation uses unknown locale `{locale}`"
        );
        validate_source_path(source);
    }
}

fn validate_source_path(source: &str) {
    let root = fs::canonicalize(".").expect("resolve repository root");
    let path = fs::canonicalize(Path::new("oeps").join(source))
        .unwrap_or_else(|_| panic!("documentation source does not exist: {source}"));
    assert!(
        path.starts_with(root),
        "documentation source escapes the repository: {source}"
    );
    assert!(
        path.is_file(),
        "documentation source is not a file: {source}"
    );
}

fn corpus_hash(documents: &[Document]) -> String {
    let mut digest = Sha256::new();
    for document in documents {
        for value in [
            document.id.as_bytes(),
            document.collection.as_bytes(),
            document.status.as_deref().unwrap_or_default().as_bytes(),
            document.hash.as_bytes(),
        ] {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value);
        }
        digest.update([u8::from(document.normative)]);
        digest.update(document.revision.to_be_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn load_document(path: &Path, id: String, collection: &str, normative: bool) -> Document {
    let markdown = fs::read_to_string(path).expect("read documentation source");
    document_from_source(path.to_path_buf(), id, collection, normative, markdown)
}

fn document_from_source(
    path: PathBuf,
    id: String,
    collection: &str,
    normative: bool,
    markdown: String,
) -> Document {
    let title = front_matter(&markdown, "title")
        .or_else(|| {
            markdown
                .lines()
                .find_map(|line| line.strip_prefix("# ").map(str::to_owned))
        })
        .unwrap_or_else(|| id.clone());
    let revision = front_matter(&markdown, "revision")
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let source = path.to_string_lossy().replace('\\', "/");
    let hash = hash_bytes(markdown.as_bytes());
    Document {
        id,
        title,
        collection: collection.to_owned(),
        normative,
        status: front_matter(&markdown, "status"),
        revision,
        markdown,
        source,
        hash,
    }
}

fn front_matter(source: &str, key: &str) -> Option<String> {
    let mut lines = source.lines();
    if lines.next()? != "---" {
        return None;
    }
    for line in lines {
        if line == "---" {
            break;
        }
        let Some((candidate, value)) = line.split_once(':') else {
            continue;
        };
        if candidate.trim() == key {
            return Some(value.trim().to_owned());
        }
    }
    None
}

fn headings(document: &Document) -> Vec<(String, String, String)> {
    let mut chunks = Vec::new();
    let mut heading = document.title.clone();
    let mut anchor = String::new();
    let mut body = String::new();
    for line in document.markdown.lines() {
        if let Some(raw) = line.strip_prefix("## ") {
            if !body.trim().is_empty() {
                chunks.push((anchor, heading, body.trim().to_owned()));
            }
            heading = raw.to_owned();
            anchor = raw
                .to_lowercase()
                .chars()
                .map(|character| {
                    if character.is_alphanumeric() {
                        character
                    } else {
                        '-'
                    }
                })
                .collect::<String>()
                .trim_matches('-')
                .to_owned();
            body.clear();
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    if !body.trim().is_empty() {
        chunks.push((anchor, heading, body.trim().to_owned()));
    }
    chunks
}

fn build_database(
    path: &Path,
    documents: &[Document],
    snapshot_hash: &str,
    publication_channel: &str,
) {
    let _ = fs::remove_file(path);
    block_on(async {
        let database = Builder::new_local(path)
            .build()
            .await
            .expect("create documentation database");
        let connection = database.connect().expect("connect documentation database");
        connection.execute_batch(
            "PRAGMA journal_mode=DELETE; PRAGMA page_size=4096; PRAGMA auto_vacuum=NONE;
             CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
             CREATE TABLE documents (id TEXT PRIMARY KEY, title TEXT NOT NULL, collection TEXT NOT NULL, normative INTEGER NOT NULL, status TEXT, revision INTEGER NOT NULL, content_hash TEXT NOT NULL, markdown TEXT NOT NULL, source TEXT NOT NULL) WITHOUT ROWID;
             CREATE TABLE chunks (document_id TEXT NOT NULL, anchor TEXT NOT NULL, heading TEXT NOT NULL, body TEXT NOT NULL, ordinal INTEGER NOT NULL, PRIMARY KEY(document_id, ordinal)) WITHOUT ROWID;
             CREATE VIRTUAL TABLE document_fts USING fts5(document_id UNINDEXED, title, heading, body, anchor UNINDEXED, tokenize='unicode61');"
        ).await.expect("create documentation schema");
        let transaction = connection
            .transaction()
            .await
            .expect("documentation transaction");
        transaction
            .execute(
                "INSERT INTO metadata VALUES ('schema_version', 'osiris.documentation/v2')",
                (),
            )
            .await
            .unwrap();
        transaction
            .execute(
                "INSERT INTO metadata VALUES ('publication_channel', ?1)",
                [publication_channel],
            )
            .await
            .unwrap();
        transaction
            .execute(
                "INSERT INTO metadata VALUES ('snapshot_hash', ?1)",
                [snapshot_hash],
            )
            .await
            .unwrap();
        for document in documents {
            transaction
                .execute(
                    "INSERT INTO documents VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        document.id.clone(),
                        document.title.clone(),
                        document.collection.clone(),
                        document.normative,
                        document.status.clone(),
                        document.revision,
                        document.hash.clone(),
                        document.markdown.clone(),
                        document.source.clone()
                    ],
                )
                .await
                .unwrap();
            for (ordinal, (anchor, heading, body)) in headings(document).into_iter().enumerate() {
                transaction
                    .execute(
                        "INSERT INTO chunks VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            document.id.clone(),
                            anchor.clone(),
                            heading.clone(),
                            body.clone(),
                            ordinal as i64
                        ],
                    )
                    .await
                    .unwrap();
                transaction
                    .execute(
                        "INSERT INTO document_fts VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            document.id.clone(),
                            document.title.clone(),
                            heading,
                            body,
                            anchor
                        ],
                    )
                    .await
                    .unwrap();
            }
        }
        transaction
            .commit()
            .await
            .expect("commit documentation snapshot");
        connection
            .execute_batch("PRAGMA optimize; VACUUM;")
            .await
            .expect("finalize documentation snapshot");
    });
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}
