use super::*;

pub(super) struct WorkspaceBuffer {
    pub(super) uri: String,
    pub(super) source: String,
    pub(super) options: CompileOptions,
}

pub(super) fn project_options(
    project: &ProjectConfig,
    path: &Path,
    module_name: String,
) -> CompileOptions {
    CompileOptions::new(&module_name, project.target_python)
        .with_source_name(path.display().to_string())
        .with_expected_module_name(module_name)
        .with_provider(
            project.distribution.clone(),
            project.distribution_version.clone(),
        )
}

pub(super) fn collect_workspace_sources(
    directory: &Path,
    paths: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_workspace_sources(&entry.path(), paths)?;
        } else if file_type.is_file()
            && entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("osr")
        {
            paths.push(entry.path());
        }
    }
    Ok(())
}

pub(super) fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let encoded = if let Some(path) = encoded.strip_prefix("localhost/") {
        format!("/{path}")
    } else if encoded.starts_with('/') {
        encoded.to_owned()
    } else {
        return None;
    };
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex_digit(*bytes.get(index + 1)?)?;
            let low = hex_digit(*bytes.get(index + 2)?)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok().map(PathBuf::from)
}

pub(super) fn load_project_interfaces(
    project: &ProjectConfig,
    site_roots: &[PathBuf],
) -> Option<BTreeMap<String, Interface>> {
    if project.extensions.is_empty() {
        return Some(BTreeMap::new());
    }
    if site_roots.is_empty() {
        return None;
    }
    let lock = project.load_lock().ok()?;
    let graph = dependency::resolve_effective_extensions(project, &lock, site_roots).ok()?;
    let mut interfaces = BTreeMap::<String, Interface>::new();
    for distribution in graph.extensions {
        for extension in distribution.extensions {
            let source = fs::read_to_string(&extension.interface).ok()?;
            let parsed = interface::read(&source).ok()?;
            if parsed.module != extension.module
                || parsed.semantic_interface_hash() != extension.semantic_interface_hash
            {
                return None;
            }
            if let Some(existing) = interfaces.get(&parsed.module) {
                if existing.semantic_interface_hash() != parsed.semantic_interface_hash() {
                    return None;
                }
            } else {
                interfaces.insert(parsed.module.clone(), parsed);
            }
        }
    }
    Some(interfaces)
}

pub(super) const fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(super) fn fallback_module_name(uri: &str) -> String {
    let path = uri.rsplit('/').next().unwrap_or(uri);
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("main")
        .to_owned()
}

pub(super) fn normalize_locale(locale: String) -> String {
    if is_chinese_locale(&locale) {
        "zh-CN".to_owned()
    } else if locale.is_empty() {
        "en".to_owned()
    } else {
        locale
    }
}

pub(super) fn is_chinese_locale(locale: &str) -> bool {
    locale.eq_ignore_ascii_case("zh")
        || locale.eq_ignore_ascii_case("zh-cn")
        || locale.to_ascii_lowercase().starts_with("zh-")
}

pub(super) fn rename_group_has_declaration(
    index: &WorkspaceSymbolIndex,
    binding_id: &str,
    spelling: &str,
) -> bool {
    let spelling = spelling.nfc().collect::<String>();
    index
        .rename_occurrences
        .get(binding_id)
        .into_iter()
        .flatten()
        .any(|occurrence| {
            occurrence.declaration && occurrence.spelling.nfc().collect::<String>() == spelling
        })
}

pub(super) fn rename_kind_supported(index: &WorkspaceSymbolIndex, binding_id: &str) -> bool {
    // The current semantic projection records every runtime value occurrence,
    // but it does not yet retain nominal type, field, module, or phase-1 macro
    // references. Refuse those categories instead of emitting a partial edit.
    matches!(
        index.binding_kinds.get(binding_id),
        Some(BindingKind::Function | BindingKind::Value | BindingKind::Parameter)
    )
}

pub(super) fn document_declares_phase_name(document: &OpenDocument, name: &str) -> bool {
    document.analysis.document.forms.iter().any(|form| {
        let FormKind::List(items) = &form.kind else {
            return false;
        };
        let Some(head) = items.first().and_then(form_name) else {
            return false;
        };
        matches!(head, "defmacro" | "defn-for-syntax")
            && items
                .get(1)
                .and_then(form_name)
                .is_some_and(|declared| declared.nfc().eq(name.nfc()))
    })
}
