use super::*;

/// Errors produced by graph construction and interface loading.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModuleGraphError {
    UnnamedModule {
        span: Span,
    },
    DuplicateModule {
        module: String,
        first: Option<Span>,
        second: Option<Span>,
    },
    MissingModule {
        from: String,
        module: String,
        kind: EdgeKind,
        span: Span,
    },
    InterfaceIo {
        requested: String,
        path: PathBuf,
        message: String,
    },
    InterfaceParse {
        requested: String,
        path: PathBuf,
        message: String,
    },
    InterfaceModuleMismatch {
        requested: String,
        declared: String,
        path: PathBuf,
    },
    DuplicateInterface {
        module: String,
        first: PathBuf,
        second: PathBuf,
    },
    Phase1Cycle {
        modules: Vec<String>,
    },
}

impl fmt::Display for ModuleGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnnamedModule { .. } => formatter.write_str("module declaration requires a name"),
            Self::DuplicateModule { module, .. } => {
                write!(formatter, "duplicate module `{module}`")
            }
            Self::MissingModule {
                from, module, kind, ..
            } => write!(
                formatter,
                "{kind} import from `{from}` references missing module `{module}`"
            ),
            Self::InterfaceIo {
                requested,
                path,
                message,
            } => write!(
                formatter,
                "could not read interface for `{requested}` at {}: {message}",
                path.display()
            ),
            Self::InterfaceParse {
                requested,
                path,
                message,
            } => write!(
                formatter,
                "invalid interface for `{requested}` at {}: {message}",
                path.display()
            ),
            Self::InterfaceModuleMismatch {
                requested,
                declared,
                path,
            } => write!(
                formatter,
                "interface path {} requested as `{requested}` declares `{declared}`",
                path.display()
            ),
            Self::DuplicateInterface {
                module,
                first,
                second,
            } => write!(
                formatter,
                "module `{module}` is provided by both {} and {}",
                first.display(),
                second.display()
            ),
            Self::Phase1Cycle { modules } => {
                write!(formatter, "phase-1 import cycle: {}", modules.join(" -> "))
            }
        }
    }
}

impl std::error::Error for ModuleGraphError {}

/// A cycle error returned by a topological ordering request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TopologyError {
    Cycle {
        components: Vec<StronglyConnectedComponent>,
    },
}

impl fmt::Display for TopologyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cycle { components } => {
                let names = components
                    .iter()
                    .map(|component| component.modules.join(" <-> "))
                    .collect::<Vec<_>>();
                write!(
                    formatter,
                    "dependency graph contains cycle(s): {}",
                    names.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for TopologyError {}

/// Read-only lookup failures for public interface members.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExportLookupError {
    MissingModule {
        module: String,
    },
    MissingExport {
        module: String,
        name: String,
    },
    WrongKind {
        module: String,
        name: String,
        expected: BindingKind,
        actual: BindingKind,
    },
    MissingInterfaceMember {
        module: String,
        binding: String,
    },
}

impl fmt::Display for ExportLookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingModule { module } => {
                write!(formatter, "module `{module}` has no loaded interface")
            }
            Self::MissingExport { module, name } => {
                write!(formatter, "module `{module}` does not export `{name}`")
            }
            Self::WrongKind {
                module,
                name,
                expected,
                actual,
            } => write!(
                formatter,
                "export `{module}/{name}` has kind {actual:?}, expected {expected:?}"
            ),
            Self::MissingInterfaceMember { module, binding } => {
                write!(
                    formatter,
                    "interface `{module}` has no member description for `{binding}`"
                )
            }
        }
    }
}

impl std::error::Error for ExportLookupError {}
