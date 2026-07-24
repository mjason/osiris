use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{EdgeKind, ModuleGraph, ModuleGraphError, TopologyError, read_interface_file};
use crate::{ast, compiler, project::PythonVersion, reader};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn source(text: &str) -> ast::Module {
    let lowered = ast::lower_document(&reader::read(text));
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    lowered.module
}

fn fixture_interface() -> (PathBuf, String) {
    let source_text = r#"
            (module dep.core)
            ^{:doc "Return an integer."}
            (defn ^Int run [^Int x] x)
            ^{:doc "A point fixture."}
            (defstruct Point [x Int])
            (alias 执行 run)
            (export [run Point 执行])
        "#;
    let options = compiler::CompileOptions::new("dep.core", PythonVersion::MINIMUM);
    let result = compiler::compile(source_text, &options);
    assert!(!result.has_errors(), "{:?}", result.analysis.diagnostics);
    let encoded = result.interface.expect("fixture should have interface");
    let path = std::env::temp_dir().join(format!(
        "osiris-module-graph-{}-{}.osri",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&path, &encoded).expect("interface fixture should be written");
    (path, encoded)
}

#[test]
fn separates_runtime_and_phase1_edges_and_orders_deterministically() {
    let graph = ModuleGraph::build([
        source("(module a) (import b) (import-for-syntax c)"),
        source("(module b) (import c)"),
        source("(module c)"),
    ])
    .expect("graph should build");
    assert_eq!(graph.runtime().edges().len(), 2);
    assert_eq!(graph.phase1().edges().len(), 1);
    assert_eq!(graph.runtime().edges()[0].kind, EdgeKind::Runtime);
    assert_eq!(graph.phase1().edges()[0].kind, EdgeKind::Phase1);
    assert_eq!(graph.runtime().dependency_order().unwrap(), ["c", "b", "a"]);
    assert_eq!(
        graph
            .runtime()
            .scc_dependency_order()
            .into_iter()
            .map(|component| component.modules)
            .collect::<Vec<_>>(),
        vec![
            vec!["c".to_owned()],
            vec!["b".to_owned()],
            vec!["a".to_owned()]
        ]
    );
    assert_eq!(
        graph.runtime().topological_order().unwrap(),
        ["a", "b", "c"]
    );
    assert_eq!(graph.phase1().sccs().len(), 3);
}

#[test]
fn detects_duplicate_missing_and_phase1_cycles() {
    let duplicate = ModuleGraph::build([source("(module same)"), source("(module same)")])
        .expect_err("duplicate should fail");
    assert!(matches!(
        duplicate,
        ModuleGraphError::DuplicateModule { .. }
    ));

    let missing = ModuleGraph::build([source("(module root) (import absent)")])
        .expect_err("missing import should fail");
    assert!(matches!(missing, ModuleGraphError::MissingModule { .. }));

    let cycle = ModuleGraph::build([
        source("(module a) (import-for-syntax b)"),
        source("(module b) (import-for-syntax a)"),
    ])
    .expect_err("phase1 cycle should fail");
    assert_eq!(
        cycle,
        ModuleGraphError::Phase1Cycle {
            modules: vec!["a".to_owned(), "b".to_owned()]
        }
    );
}

#[test]
fn runtime_cycle_is_reported_by_topology_but_allowed_in_graph() {
    let graph = ModuleGraph::build([
        source("(module a) (import b)"),
        source("(module b) (import a)"),
    ])
    .expect("runtime cycles are allowed");
    let error = graph
        .runtime()
        .dependency_order()
        .expect_err("cycle expected");
    assert!(matches!(error, TopologyError::Cycle { .. }));
    assert_eq!(graph.runtime().sccs()[0].modules, ["a", "b"]);
    assert_eq!(
        graph.runtime().scc_dependency_order()[0].modules,
        ["a", "b"]
    );
}

#[test]
fn runtime_scc_order_is_dependency_first_across_components() {
    let graph = ModuleGraph::build([
        source("(module app) (import left) (import right)"),
        source("(module left) (import shared) (import cycle.one)"),
        source("(module right) (import shared)"),
        source("(module shared)"),
        source("(module cycle.one) (import cycle.two)"),
        source("(module cycle.two) (import cycle.one) (import shared)"),
    ])
    .expect("runtime graph should build");
    let order = graph
        .runtime()
        .scc_dependency_order()
        .into_iter()
        .map(|component| component.modules)
        .collect::<Vec<_>>();
    assert_eq!(
        order,
        vec![
            vec!["shared".to_owned()],
            vec!["cycle.one".to_owned(), "cycle.two".to_owned()],
            vec!["left".to_owned()],
            vec!["right".to_owned()],
            vec!["app".to_owned()],
        ]
    );
}

#[test]
fn loads_interface_without_executing_python_and_resolves_aliases() {
    let (path, encoded) = fixture_interface();
    let interface = read_interface_file("dep.core", &path).expect("interface should load");
    assert_eq!(interface.module, "dep.core");
    assert!(encoded.contains("osiris-interface"));

    let mut paths = std::collections::BTreeMap::new();
    paths.insert("dep.core".to_owned(), path.clone());
    let graph = ModuleGraph::build_with_interface_paths(
        [source("(module app) (import dep.core :refer [执行 Point])")],
        &paths,
    )
    .expect("external interface should satisfy import");
    assert_eq!(
        graph.exported_function("dep.core", "执行").unwrap().binding,
        "dep.core::function::run"
    );
    assert_eq!(
        graph.exported_alias("dep.core", "执行").unwrap().target,
        "dep.core::function::run"
    );
    assert_eq!(
        graph.exported_struct("dep.core", "Point").unwrap().binding,
        "dep.core::type::Point"
    );
    let import = match &graph.source_modules()["app"].items[0].kind {
        ast::ItemKind::Import(import) => import,
        _ => panic!("expected import"),
    };
    let resolved = graph.resolve_import("app", import).unwrap();
    assert_eq!(resolved.members.len(), 2);
    let _ = fs::remove_file(path);
}

#[test]
fn rejects_interface_module_mismatch() {
    let (path, _) = fixture_interface();
    let error = read_interface_file("other", &path).expect_err("mismatch should fail");
    assert!(matches!(
        error,
        ModuleGraphError::InterfaceModuleMismatch { .. }
    ));
    let _ = fs::remove_file(path);
}
