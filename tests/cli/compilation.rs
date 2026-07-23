use super::*;

#[test]
fn compile_writes_parseable_python_and_source_map_atomically() {
    let fixture = SourceFixture::new(
        "(module sample)\n\
         (export [square answer])\n\
         (defn square [[x Float]] -> Float (* x x))\n\
         (def answer Float (square 3.0))\n",
    );
    let out_dir = fixture.directory.join("build");
    let output = osr(&[
        "compile",
        path_argument(&fixture.path),
        "--out-dir",
        path_argument(&out_dir),
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let python = out_dir.join("sample.py");
    let interface = out_dir.join("sample.osri");
    let source_map = out_dir.join("sample.py.map");
    assert!(python.is_file());
    assert!(interface.is_file());
    assert!(source_map.is_file());
    let interface_text = fs::read_to_string(interface).expect("interface should be readable");
    assert!(interface_text.starts_with("(osiris-interface"));
    assert!(interface_text.contains("sha256:"));
    let syntax = Command::new("python3")
        .args(["-m", "py_compile", path_argument(&python)])
        .output()
        .expect("Python should validate generated source");
    assert!(
        syntax.status.success(),
        "{}",
        String::from_utf8_lossy(&syntax.stderr)
    );
    let map: serde_json::Value =
        serde_json::from_slice(&fs::read(source_map).expect("source map should be readable"))
            .expect("source map should be JSON");
    assert_eq!(map["version"], 1);
    assert_eq!(map["generated"], "sample.py");
    assert!(
        map["mappings"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
}

#[test]
fn compile_project_entry_discovers_and_emits_dependency_modules() {
    let fixture = SourceFixture::new("(def ignored 0)\n");
    let app = fixture.write(
        "src/demo/app.osr",
        r#"(module demo.app)
            (import demo.math :as math)
            (export [answer])
            (def answer (math/add-one 41))
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
        "[project]\nname = \"workspace-build\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
    )
    .expect("project configuration should be written");
    let out_dir = fixture.directory.join("workspace-build");

    let output = osr(&[
        "compile",
        path_argument(&app),
        "--out-dir",
        path_argument(&out_dir),
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out_dir.join("demo/app.py").is_file());
    assert!(out_dir.join("demo/math.py").is_file());
    assert!(out_dir.join("demo/app.osri").is_file());
    assert!(out_dir.join("demo/math.osri").is_file());
}

#[test]
fn compile_emits_canonical_public_records_manifest() {
    let fixture = SourceFixture::new(
        r#"(module sample)
            (export [owner S])
            (defstatic-schema S
              :schema-id "sample/schema"
              :version 1
              :fields {:id {:type Str :required true}})
            (def owner none)
            (static-record S owner {:id "alpha"})
        "#,
    );
    let out_dir = fixture.directory.join("records-build");
    let output = osr(&[
        "compile",
        path_argument(&fixture.path),
        "--out-dir",
        path_argument(&out_dir),
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let sidecars = fs::read_dir(&out_dir)
        .expect("output directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.to_string_lossy().ends_with(".records.json"))
        .collect::<Vec<_>>();
    assert_eq!(sidecars.len(), 1);
    let bytes = fs::read(&sidecars[0]).expect("records manifest should be readable");
    let manifest: serde_json::Value =
        serde_json::from_slice(&bytes).expect("records manifest should be JSON");
    assert_eq!(manifest["format-version"], 1);
    assert_eq!(manifest["records"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        bytes.last(),
        Some(&b'}'),
        "canonical JSON has no trailing newline"
    );
}

#[test]
fn compile_aggregates_multiple_modules_into_one_distribution_manifest() {
    let first_source = r#"(module sample.first)
            (export [owner FirstSchema])
            (defstatic-schema FirstSchema
              :schema-id "sample/first"
              :version 1
              :fields {:id {:type Str :required true}})
            (def owner none)
            (static-record FirstSchema owner {:id "first"})
        "#;
    let fixture = SourceFixture::new(first_source);
    let first = fixture.write("src/sample/first.osr", first_source);
    let second = fixture.write(
        "src/sample/second.osr",
        r#"(module sample.second)
            (export [owner SecondSchema])
            (defstatic-schema SecondSchema
              :schema-id "sample/second"
              :version 1
              :fields {:id {:type Str :required true}})
            (def owner none)
            (static-record SecondSchema owner {:id "second"})
        "#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"demo-osiris\"\nversion = \"1.2.3\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
    )
    .expect("project configuration should be written");
    let out_dir = fixture.directory.join("distribution-build");
    let output = osr(&[
        "compile",
        path_argument(&first),
        path_argument(&second),
        "--out-dir",
        path_argument(&out_dir),
        "--emit",
        "py,osri,map,records",
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out_dir.join("sample/first.py").is_file());
    assert!(out_dir.join("sample/second.py").is_file());
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(out_dir.join("demo-osiris.records.json"))
            .expect("distribution manifest should exist"),
    )
    .expect("distribution manifest should be JSON");
    assert_eq!(manifest["records"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        manifest["interface-semantic-hashes"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert!(
        manifest["records"]
            .as_array()
            .expect("records array")
            .iter()
            .all(
                |record| record["occurrence"]["distribution"] == "demo-osiris"
                    && record["occurrence"]["version"] == "1.2.3"
            )
    );
}

#[test]
fn compile_orders_sources_and_replays_dependency_macro_ir() {
    let app_source = r#"(module sample.app)
            (import sample.macros :as macros)
            (export [increment])
            (defn increment [[value Int]] -> Int
              (macros/add-one value))
        "#;
    let fixture = SourceFixture::new(app_source);
    let app = fixture.write("src/sample/app.osr", app_source);
    let macros = fixture.write(
        "src/sample/macros.osr",
        r#"(module sample.macros)
            (defn-for-syntax make-add [value]
              (list '+ value 1))
            (defmacro add-one [value]
              (make-add value))
            (export [add-one])
        "#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"macro-demo\"\nversion = \"1.0.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
    )
    .expect("project configuration should be written");
    let out_dir = fixture.directory.join("macro-build");

    let output = osr(&[
        "compile",
        path_argument(&app),
        path_argument(&macros),
        "--out-dir",
        path_argument(&out_dir),
        "--emit",
        "py,osri,map",
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let generated = fs::read_to_string(out_dir.join("sample/app.py"))
        .expect("macro consumer Python should exist");
    assert!(generated.contains("return value + 1"), "{generated}");
    let interface = fs::read_to_string(out_dir.join("sample/macros.osri"))
        .expect("macro interface should exist");
    assert!(interface.contains("add-one"));
    assert!(interface.contains("make-add"));
}

#[test]
fn compile_consumes_a_locked_static_extension_interface() {
    const SOURCE_HASH: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    let fixture = SourceFixture::new(
        r#"(module demo.app)
            (import sample.core :as sample)
            (export [answer])
            (defn answer [] -> Int (sample/add 40 2))
        "#,
    );
    let extension_source = fixture.directory.join("sample-extension.osr");
    fs::write(
        &extension_source,
        r#"(module sample.core)
            (export [add])
            (defn add [[left Int] [right Int]] -> Int (+ left right))
        "#,
    )
    .expect("extension source should be written");
    let site_root = fixture.directory.join("site");
    let extension_package = site_root.join("sample_ext");
    let extension_output = osr(&[
        "compile",
        path_argument(&extension_source),
        "--out-dir",
        path_argument(&extension_package),
        "--emit",
        "osri",
    ]);
    assert!(
        extension_output.status.success(),
        "{}",
        String::from_utf8_lossy(&extension_output.stderr)
    );

    let dist_info = site_root.join("sample_ext-1.0.dist-info");
    fs::create_dir_all(&dist_info).expect("dist-info should be created");
    fs::write(
        dist_info.join("METADATA"),
        "Metadata-Version: 2.4\nName: sample-ext\nVersion: 1.0\n\n",
    )
    .expect("extension metadata should be written");
    fs::write(
        dist_info.join("osiris.toml"),
        format!(
            "schema = 1\ncompiler_abi = 1\nlanguage_abi = 2\nsource_hash = \"{SOURCE_HASH}\"\n\n[[extension]]\nid = \"sample\"\ninterface = \"sample_ext/sample/core.osri\"\n"
        ),
    )
    .expect("extension marker should be written");
    fs::write(
        fixture.directory.join("pyproject.toml"),
        r#"[project]
name = "demo"
version = "1.0"

[tool.osiris]
extensions = ["sample"]
"#,
    )
    .expect("project configuration should be written");
    fs::write(
        fixture.directory.join("uv.lock"),
        format!(
            r#"version = 1

[[package]]
name = "demo"
source = {{ editable = "." }}

[[package]]
name = "sample-ext"
version = "1.0"
source = {{ registry = "https://pypi.org/simple", hash = "{SOURCE_HASH}" }}
"#
        ),
    )
    .expect("uv lock should be written");

    let app = fixture.write(
        "src/demo/app.osr",
        &fs::read_to_string(&fixture.path).expect("fixture source should be readable"),
    );

    let out_dir = fixture.directory.join("app-build");
    let output = osr(&[
        "compile",
        path_argument(&app),
        "--site-root",
        path_argument(&site_root),
        "--out-dir",
        path_argument(&out_dir),
        "--emit",
        "py,osri",
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let generated =
        fs::read_to_string(out_dir.join("demo/app.py")).expect("application Python should exist");
    assert!(
        generated.contains("from sample.core import add"),
        "{generated}"
    );
    assert!(generated.contains("return add(40, 2)"), "{generated}");
}

#[test]
fn project_compile_rejects_a_module_name_that_disagrees_with_its_path() {
    let fixture = SourceFixture::new("(def ignored 0)\n");
    let source = fixture.write(
        "src/demo/actual.osr",
        "(module demo.other)\n(def value 1)\n",
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"1.0\"\n\n[tool.osiris]\nsource = [\"src\"]\n",
    )
    .expect("project configuration should be written");
    let out_dir = fixture.directory.join("mismatch-build");

    let output = osr(&[
        "compile",
        path_argument(&source),
        "--out-dir",
        path_argument(&out_dir),
    ]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("OSR-G0011"), "{stderr}");
    assert!(stderr.contains("demo.actual"), "{stderr}");
    assert!(!out_dir.exists());
}
