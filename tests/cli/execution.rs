use super::*;

#[test]
fn run_executes_fully_expanded_threading_pipeline() {
    let fixture = SourceFixture::new(
        "(py/import builtins :as py)\n\
         (defn calculate [[x Int]] -> Int (-> x (+ 1) (* 2)))\n\
         (py.print (calculate 20))\n",
    );
    let output = osr(&["run", path_argument(&fixture.path)]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n");
}

#[test]
fn run_compiles_the_project_workspace_before_executing_the_entry() {
    let fixture = SourceFixture::new("(def ignored 0)\n");
    let app = fixture.write(
        "src/demo/app.osr",
        r#"(module demo.app)
            (import demo.math :as math)
            (py/import builtins :as py)
            (py.print (math/add-one 41))
        "#,
    );
    fixture.write(
        "src/demo/math.osr",
        r#"(module demo.math)
            (export [add-one])
            (defn add-one [[value Int]] -> Int (+ value 1))
        "#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"workspace-run\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
    )
    .expect("project configuration should be written");

    let output = osr(&["run", path_argument(&app)]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n");
}

struct RecordsResolverFixture {
    _fixture: SourceFixture,
    app: PathBuf,
    site_root: PathBuf,
    records_path: PathBuf,
    marker_path: PathBuf,
}

fn records_resolver_fixture() -> RecordsResolverFixture {
    const SOURCE_HASH: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let fixture = SourceFixture::new(
        r#"(module demo.app)
            (py/import builtins :as py)
            (py/import os :as os)
            (py.print (os.path.isfile (os.getenv "OSIRIS_RECORDS_RESOLVER")))
        "#,
    );
    let extension_root = fixture.directory.join("extension");
    let extension_source = extension_root.join("src/sample/core.osr");
    fs::create_dir_all(extension_source.parent().expect("extension source parent"))
        .expect("extension source parent should be created");
    fs::write(
        extension_root.join("pyproject.toml"),
        "[project]\nname = \"sample-ext\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
    )
    .expect("extension project configuration should be written");
    fs::write(
        &extension_source,
        r#"(module sample.core)
            (export [owner S])
            (defstatic-schema S
              :schema-id "sample/schema"
              :version 1
              :fields {:id {:type Str :required true}})
            (def owner none)
            (static-record S owner {:id "alpha"})
        "#,
    )
    .expect("extension source should be written");
    fs::write(
        extension_root.join("src/sample/extra.osr"),
        r#"(module sample.extra)
            (export [value])
            (defn value [] -> Int 1)
        "#,
    )
    .expect("second extension interface source should be written");
    let site_root = fixture.directory.join("site");
    let extension_output = site_root.join("sample_ext");
    let extension_build = osr(&[
        "compile",
        path_argument(&extension_source),
        "--out-dir",
        path_argument(&extension_output),
        "--emit",
        "py,osri,records",
    ]);
    assert!(
        extension_build.status.success(),
        "{}",
        String::from_utf8_lossy(&extension_build.stderr)
    );
    let records_path = extension_output.join("sample-ext.records.json");
    let records_bytes = fs::read(&records_path).expect("extension records sidecar should exist");
    let records_hash = format!("sha256:{:x}", sha2::Sha256::digest(&records_bytes));
    let dist_info = site_root.join("sample_ext-1.0.dist-info");
    fs::create_dir_all(&dist_info).expect("dist-info should be created");
    fs::write(
        dist_info.join("METADATA"),
        "Metadata-Version: 2.4\nName: sample-ext\nVersion: 1.0\n\n",
    )
    .expect("extension metadata should be written");
    let marker_path = dist_info.join("osiris.toml");
    fs::write(
        &marker_path,
        format!(
            "schema = 1\ncompiler_abi = 1\nlanguage_abi = 2\nsource_hash = \"{SOURCE_HASH}\"\nrecords = \"sample_ext/sample-ext.records.json\"\nrecords_hash = \"{records_hash}\"\n\n[[extension]]\nid = \"sample\"\ninterface = \"sample_ext/sample/core.osri\"\n\n[[extension]]\nid = \"sample-extra\"\ninterface = \"sample_ext/sample/extra.osri\"\n"
        ),
    )
    .expect("extension marker should be written");
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\nextensions = [\"sample\"]\n",
    )
    .expect("project configuration should be written");
    fs::write(
        fixture.directory.join("uv.lock"),
        format!(
            "version = 1\n\n[[package]]\nname = \"demo\"\nsource = {{ editable = \".\" }}\n\n[[package]]\nname = \"sample-ext\"\nversion = \"1.0\"\nsource = {{ registry = \"https://pypi.org/simple\", hash = \"{SOURCE_HASH}\" }}\n"
        ),
    )
    .expect("uv lock should be written");
    let app = fixture.write(
        "src/demo/app.osr",
        r#"(module demo.app)
            (py/import builtins :as py)
            (py/import os :as os)
            (py.print (os.path.isfile (os.getenv "OSIRIS_RECORDS_RESOLVER")))
        "#,
    );
    RecordsResolverFixture {
        _fixture: fixture,
        app,
        site_root,
        records_path,
        marker_path,
    }
}

fn rewrite_external_record_occurrence(
    prepared: &RecordsResolverFixture,
    mut mutate: impl FnMut(&mut records::RecordOccurrenceId),
) {
    let bytes = fs::read(&prepared.records_path).expect("records sidecar should be readable");
    let sidecar = records::decode_sidecar(&bytes, None).expect("sidecar should decode");
    let interface_hashes = sidecar.interface_semantic_hashes.clone();
    let indexed = sidecar
        .records
        .into_iter()
        .map(|mut entry| {
            mutate(&mut entry.occurrence);
            records::IndexedRecord {
                occurrence: entry.occurrence,
                record: entry.record,
                dependency_path: Vec::new(),
            }
        })
        .collect::<Vec<_>>();
    let encoded = records::encode_sidecar(interface_hashes, indexed)
        .expect("tampered sidecar should remain structurally valid");
    fs::write(&prepared.records_path, &encoded.bytes).expect("tampered sidecar should be written");
    let marker = fs::read_to_string(&prepared.marker_path).expect("marker should be readable");
    let marker = marker
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("records_hash") {
                format!("records_hash = \"{}\"", encoded.records_hash)
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&prepared.marker_path, format!("{marker}\n"))
        .expect("updated marker should be written");
}

fn run_records_resolver_fixture(prepared: &RecordsResolverFixture) -> Output {
    osr(&[
        "run",
        path_argument(&prepared.app),
        "--site-root",
        path_argument(&prepared.site_root),
    ])
}

#[test]
fn run_resolves_one_enabled_interface_from_a_multi_interface_wheel_sidecar() {
    let prepared = records_resolver_fixture();
    let output = osr(&[
        "run",
        path_argument(&prepared.app),
        "--site-root",
        path_argument(&prepared.site_root),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "True\n");
}

#[test]
fn run_rejects_external_records_version_mismatch() {
    let prepared = records_resolver_fixture();
    rewrite_external_record_occurrence(&prepared, |occurrence| {
        occurrence.version = "9.9".to_owned();
    });
    let output = run_records_resolver_fixture(&prepared);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("version mismatch"), "{stderr}");
}

#[test]
fn run_rejects_external_records_interface_member_mismatch() {
    let prepared = records_resolver_fixture();
    rewrite_external_record_occurrence(&prepared, |occurrence| {
        occurrence.interface_member_id = "sample.other".to_owned();
    });
    let output = run_records_resolver_fixture(&prepared);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("interface-member-id mismatch"), "{stderr}");
}

#[test]
fn run_rejects_external_records_semantic_hash_mismatch() {
    let prepared = records_resolver_fixture();
    rewrite_external_record_occurrence(&prepared, |occurrence| {
        occurrence.semantic_interface_hash =
            "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_owned();
    });
    let output = run_records_resolver_fixture(&prepared);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("semantic interface hash mismatch"),
        "{stderr}"
    );
}

#[test]
fn expand_prints_threading_macro_result() {
    let fixture = SourceFixture::new("(-> value (normalize 1) finish)\n");
    let output = osr(&["expand", path_argument(&fixture.path)]);

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "(finish (normalize value 1))\n"
    );
}
