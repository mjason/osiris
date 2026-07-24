use super::*;

#[path = "compilation/extensions.rs"]
mod extensions;

#[test]
fn build_compiles_the_jsonc_project_without_a_source_argument() {
    let fixture = SourceFixture::new("none\n");
    fixture.write("src/main.osr", "(module main)\n(def value 1)\n");
    fixture.write("src/lib/value.osr", "(module lib.value)\n(def value 2)\n");
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"build-project\"\nversion = \"1.0\"\n",
    )
    .unwrap();
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{"source":["src"],"outDir":"dist","targetPython":"3.11"}"#,
    )
    .unwrap();

    let output = osr(&["build", path_argument(&fixture.directory)]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fixture.directory.join("dist/main.py").is_file());
    assert!(fixture.directory.join("dist/lib/value.py").is_file());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n", fixture.directory.join("dist").display())
    );
}

#[test]
fn project_config_controls_glob_exclusions_and_output() {
    let fixture = SourceFixture::new("none\n");
    let app = fixture.write("src/main.osr", "(module main)\n(def value 1)\n");
    fixture.write(
        "src/pkg/generated/broken.osr",
        "(module pkg.generated.broken\n",
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"configured\"\nversion = \"1\"\n",
    )
    .unwrap();
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{
          // The excluded invalid module must not enter compilation.
          "source": ["src"],
          "exclude": ["src/**/generated/**"],
          "outDir": "build/python",
          "targetPython": "3.11",
        }"#,
    )
    .unwrap();

    let output = osr(&["compile", path_argument(&app)]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = fixture.directory.join("build/python");
    assert!(out.join("main.py").is_file());
    assert!(out.join("main.osri").is_file());
    assert!(out.join("main.py.map").is_file());
    assert!(!out.join("pkg/generated/broken.py").exists());
}

#[test]
fn compile_writes_parseable_python_and_source_map_atomically() {
    let fixture = SourceFixture::new(
        "(module sample)\n\
         (export [square answer])\n\
         ^{:doc \"Square a floating-point value.\"}\n\
         (defn ^Float square [^Float x] (* x x))\n\
         ^{:doc \"The computed answer.\"}\n\
         (def ^Float answer (square 3.0))\n",
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
    assert_eq!(map["version"], 3);
    assert_eq!(map["language_version"], osiris::LANGUAGE_VERSION);
    assert_eq!(map["python_target"], "3.11");
    assert!(
        map["source_hash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
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
            ^{:doc "The computed answer."}
            (def ^Int answer (math/add-one 41))
        "#,
    );
    fixture.write(
        "src/demo/math.osr",
        r#"(module demo.math)
            (export [add-one])
            ^{:doc "Increment an integer."}
            (defn ^Int add-one [^Int value] (+ value 1))
        "#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"workspace-build\"\nversion = \"1.0\"\n",
    )
    .expect("project configuration should be written");
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{"source":["src"]}"#,
    )
    .expect("Osiris configuration should be written");
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
            ^{:doc "Schema S."}
            (defstatic-schema S
              :schema-id "sample/schema"
              :version 1
              :fields {:id {:type Str :required true}})
            ^{:doc "Record owner."}
            (def ^Any owner none)
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
            ^{:doc "First record schema."}
            (defstatic-schema FirstSchema
              :schema-id "sample/first"
              :version 1
              :fields {:id {:type Str :required true}})
            ^{:doc "First record owner."}
            (def ^Any owner none)
            (static-record FirstSchema owner {:id "first"})
        "#;
    let fixture = SourceFixture::new(first_source);
    let first = fixture.write("src/sample/first.osr", first_source);
    let second = fixture.write(
        "src/sample/second.osr",
        r#"(module sample.second)
            (export [owner SecondSchema])
            ^{:doc "Second record schema."}
            (defstatic-schema SecondSchema
              :schema-id "sample/second"
              :version 1
              :fields {:id {:type Str :required true}})
            ^{:doc "Second record owner."}
            (def ^Any owner none)
            (static-record SecondSchema owner {:id "second"})
        "#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"demo-osiris\"\nversion = \"1.2.3\"\n",
    )
    .expect("project configuration should be written");
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{"source":["src"]}"#,
    )
    .expect("Osiris configuration should be written");
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
            ^{:doc "Increment an integer."}
            (defn ^Int increment [^Int value]
              (macros/add-one value))
        "#;
    let fixture = SourceFixture::new(app_source);
    let app = fixture.write("src/sample/app.osr", app_source);
    let macros = fixture.write(
        "src/sample/macros.osr",
        r#"(module sample.macros)
            (defn-for-syntax make-add [value]
              (list '+ value 1))
            ^{:doc "Increment a syntax value."}
            (defmacro add-one [value]
              (make-add value))
            (export [add-one])
        "#,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        "[project]\nname = \"macro-demo\"\nversion = \"1.0.0\"\n",
    )
    .expect("project configuration should be written");
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{"source":["src"]}"#,
    )
    .expect("Osiris configuration should be written");
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
