use super::*;

/// Read all interfaces in a deterministic explicit path map.
pub fn read_interface_paths(
    paths: &BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<String, Interface>, ModuleGraphError> {
    let mut output = BTreeMap::new();
    let mut origins = BTreeMap::<String, PathBuf>::new();
    for (requested, path) in paths {
        let text = fs::read_to_string(path).map_err(|error| ModuleGraphError::InterfaceIo {
            requested: requested.clone(),
            path: path.clone(),
            message: error.to_string(),
        })?;
        let parsed = interface::read(&text).map_err(|error| ModuleGraphError::InterfaceParse {
            requested: requested.clone(),
            path: path.clone(),
            message: error.to_string(),
        })?;
        if parsed.module != *requested {
            return Err(ModuleGraphError::InterfaceModuleMismatch {
                requested: requested.clone(),
                declared: parsed.module.clone(),
                path: path.clone(),
            });
        }
        if let Some(first) = origins.insert(parsed.module.clone(), path.clone()) {
            return Err(ModuleGraphError::DuplicateInterface {
                module: parsed.module,
                first,
                second: path.clone(),
            });
        }
        output.insert(requested.clone(), parsed);
    }
    Ok(output)
}

/// Read one interface and verify that its declared module equals `requested`.
pub fn read_interface_file(requested: &str, path: &Path) -> Result<Interface, ModuleGraphError> {
    let mut paths = BTreeMap::new();
    paths.insert(requested.to_owned(), path.to_path_buf());
    read_interface_paths(&paths).and_then(|mut interfaces| {
        interfaces
            .remove(requested)
            .ok_or_else(|| ModuleGraphError::InterfaceParse {
                requested: requested.to_owned(),
                path: path.to_path_buf(),
                message: "interface was not returned by reader".to_owned(),
            })
    })
}

/// A small builder for callers that discover sources and interface paths
/// incrementally.
#[derive(Clone, Debug, Default)]
pub struct ModuleGraphBuilder {
    modules: Vec<ast::Module>,
    interface_paths: BTreeMap<String, PathBuf>,
}

impl ModuleGraphBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_module(&mut self, module: ast::Module) -> &mut Self {
        self.modules.push(module);
        self
    }

    pub fn add_interface_path(
        &mut self,
        module: impl Into<String>,
        path: impl Into<PathBuf>,
    ) -> &mut Self {
        self.interface_paths.insert(module.into(), path.into());
        self
    }

    pub fn build(self) -> Result<ModuleGraph, ModuleGraphError> {
        ModuleGraph::build_with_interface_paths(self.modules, &self.interface_paths)
    }
}

impl From<io::Error> for ModuleGraphError {
    fn from(error: io::Error) -> Self {
        Self::InterfaceIo {
            requested: String::new(),
            path: PathBuf::new(),
            message: error.to_string(),
        }
    }
}
