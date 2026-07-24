use super::*;

#[test]
fn compile_consumes_a_locked_static_extension_interface() {
    const SOURCE_HASH: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    let fixture = SourceFixture::new(
        r#"(module demo.app)
            (import sample.core :as sample)
            (export [answer])
            ^{:doc "The computed answer."}
            (defn ^Int answer [] (sample/add 40 2))
        "#,
    );
    let extension_source = fixture.directory.join("sample-extension.osr");
    fs::write(
        &extension_source,
        r#"(module sample.core)
            (export [add])
            ^{:doc "Add two integers."}
            (defn ^Int add [^Int left ^Int right] (+ left right))
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
    fs::copy(
        &extension_source,
        site_root.join("sample_ext/sample/core.osr"),
    )
    .expect("extension source should be packaged");
    write_extension_marker(
        &site_root,
        &dist_info,
        "sample-ext",
        "1.0",
        &[],
        &[ExtensionMarkerMember {
            id: "sample",
            interface: "sample_ext/sample/core.osri",
            source: "sample_ext/sample/core.osr",
        }],
        None,
    );
    fs::write(
        fixture.directory.join("pyproject.toml"),
        r#"[project]
name = "demo"
version = "1.0"
dependencies = ["sample-ext==1.0"]
"#,
    )
    .expect("project configuration should be written");
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{"source":["src"]}"#,
    )
    .expect("Osiris configuration should be written");
    fs::write(
        fixture.directory.join("uv.lock"),
        format!(
            r#"version = 1

[[package]]
name = "demo"
source = {{ editable = "." }}
dependencies = [{{ name = "sample-ext", version = "1.0" }}]

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
        "[project]\nname = \"demo\"\nversion = \"1.0\"\n",
    )
    .expect("project configuration should be written");
    fs::write(
        fixture.directory.join("osiris.jsonc"),
        r#"{"source":["src"]}"#,
    )
    .expect("Osiris configuration should be written");
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
