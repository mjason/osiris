use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use crate::{backend, compiler::python_module_path, python_ast as py};

const EVALUATION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESULT_BYTES: u64 = 1024 * 1024;
const MAX_STDERR_BYTES: u64 = 16 * 1024;
const RESULT_NAME: &str = "__osiris_lsa_evaluated_result_7d10c8c5";
static NEXT_EVALUATION_ID: AtomicU64 = AtomicU64::new(0);

pub(super) fn evaluate(
    workspace: &crate::cli::EvaluationWorkspace,
) -> Result<Option<serde_json::Value>, String> {
    let directory = EvaluationDirectory::create()?;
    let result_path = directory.path.join("result.json");
    let stderr_path = directory.path.join("stderr.txt");
    let records = crate::cli::stage_evaluation_records(workspace, &directory.path)?;
    let mut entry_module = None;
    let mut runtime_packages = BTreeMap::<String, backend::RuntimeSupport>::new();
    for (index, result) in workspace.result.units.iter().enumerate() {
        for embedded in &result.analysis.embedded_python {
            let destination = directory
                .path
                .join(python_module_path(&embedded.logical_module));
            write_staged(&destination, &embedded.source, "embedded Python")?;
        }
        let generated = result
            .python
            .as_ref()
            .ok_or_else(|| "compiler produced no Python output".to_owned())?;
        let module_path = directory
            .path
            .join(python_module_path(&result.analysis.hir.name));
        let source = if index == workspace.entry_index {
            entry_module = Some(crate::name::python_module_identifier(
                &result.analysis.hir.name,
            ));
            instrument(generated)?
        } else {
            generated.source.clone()
        };
        write_staged(&module_path, &source, "generated Python")?;
        if let Some(runtime) = &generated.runtime_support {
            let support = runtime_packages
                .entry(runtime.package.clone())
                .or_insert_with(|| backend::RuntimeSupport {
                    package: runtime.package.clone(),
                    helpers: BTreeSet::new(),
                    binding_ids: BTreeSet::new(),
                });
            support.helpers.extend(runtime.helpers.iter().cloned());
            support
                .binding_ids
                .extend(runtime.binding_ids.iter().cloned());
        }
    }
    for support in runtime_packages.values() {
        for (path, source) in backend::runtime_distribution_files(support, workspace.target_python)?
        {
            let destination = directory.path.join(path);
            write_staged(&destination, &source, "runtime support")?;
        }
    }
    let entry_module =
        entry_module.ok_or_else(|| "evaluation workspace has no entry module".to_owned())?;

    let stderr = fs::File::create(&stderr_path)
        .map_err(|error| format!("could not create evaluation stderr: {error}"))?;
    let mut command = Command::new("python3");
    command
        .args(["-m", &entry_module])
        .current_dir(&directory.path)
        .env_clear()
        .env("PATH", env::var_os("PATH").unwrap_or_default())
        .env("PYTHONPATH", &directory.path)
        .env("PYTHONUTF8", "1")
        .env("OSIRIS_LSA_RESULT", &result_path)
        .env("OSIRIS_PROJECT_RECORDS", &records.records_path)
        .env("OSIRIS_RECORDS_RESOLVER", &records.resolver_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr));
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start project Python: {error}"))?;
    let deadline = Instant::now() + EVALUATION_TIMEOUT;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("could not inspect Python evaluation: {error}"))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "Python evaluation exceeded {} seconds",
                EVALUATION_TIMEOUT.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(10));
    };
    if !status.success() {
        let stderr = read_bounded(&stderr_path, MAX_STDERR_BYTES)
            .unwrap_or_else(|_| "Python exited without readable diagnostics".to_owned());
        return Err(format!(
            "Python evaluation failed with {}: {}",
            status,
            stderr.trim()
        ));
    }
    let source = read_bounded(&result_path, MAX_RESULT_BYTES)
        .map_err(|error| format!("could not read evaluation result: {error}"))?;
    let envelope: EvaluationEnvelope = serde_json::from_str(&source)
        .map_err(|error| format!("invalid evaluation result: {error}"))?;
    Ok(envelope.result)
}

fn write_staged(path: &Path, source: &str, kind: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{kind} path has no parent"))?;
    fs::create_dir_all(parent)
        .and_then(|()| fs::write(path, source))
        .map_err(|error| format!("could not stage {kind}: {error}"))
}

fn instrument(generated: &backend::GeneratedPython) -> Result<String, String> {
    let mut module = generated.module.clone();
    let mut captured = false;
    if let Some(statement) = module
        .body
        .iter_mut()
        .rfind(|statement| matches!(statement, py::Stmt::Expr(_)))
    {
        let py::Stmt::Expr(expression) = statement else {
            unreachable!("rfind selected an expression statement")
        };
        let expression = expression.clone();
        *statement = py::Stmt::Assign(py::Assign {
            targets: vec![py::Expr::name(RESULT_NAME)],
            value: expression,
        });
        captured = true;
    }
    let mut source = module
        .to_source()
        .map_err(|error| format!("could not render evaluated Python: {error}"))?;
    source.push_str("\nimport json as __osiris_lsa_json\nimport os as __osiris_lsa_os\n");
    source.push_str("\ndef __osiris_lsa_result(value):\n");
    source.push_str("    try:\n");
    source.push_str("        __osiris_lsa_json.dumps(value, ensure_ascii=False)\n");
    source.push_str("        return value\n");
    source.push_str("    except (TypeError, ValueError):\n");
    source.push_str("        return repr(value)\n");
    source.push_str("\nwith open(__osiris_lsa_os.environ[\"OSIRIS_LSA_RESULT\"], \"w\", encoding=\"utf-8\") as __osiris_lsa_output:\n");
    if captured {
        source.push_str(&format!(
            "    __osiris_lsa_json.dump({{\"result\": __osiris_lsa_result({RESULT_NAME})}}, __osiris_lsa_output, ensure_ascii=False)\n"
        ));
    } else {
        source.push_str(
            "    __osiris_lsa_json.dump({\"result\": None}, __osiris_lsa_output, ensure_ascii=False)\n",
        );
    }
    backend::format_embedded_module(&source)
        .map_err(|error| format!("could not format evaluated Python: {error}"))
}

fn read_bounded(path: &Path, limit: u64) -> Result<String, std::io::Error> {
    let mut output = String::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_string(&mut output)?;
    if output.len() as u64 > limit {
        return Err(std::io::Error::other("file exceeded evaluation limit"));
    }
    Ok(output)
}

#[derive(serde::Deserialize)]
struct EvaluationEnvelope {
    result: Option<serde_json::Value>,
}

struct EvaluationDirectory {
    path: PathBuf,
}

impl EvaluationDirectory {
    fn create() -> Result<Self, String> {
        let id = NEXT_EVALUATION_ID.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!("osiris-lsa-eval-{}-{id}", std::process::id()));
        fs::create_dir(&path)
            .map_err(|error| format!("could not create evaluation directory: {error}"))?;
        Ok(Self { path })
    }
}

impl Drop for EvaluationDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
