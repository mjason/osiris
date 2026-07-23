#[derive(Debug)]
pub enum ConfigError {
    NotFound(PathBuf),
    MissingTable(PathBuf),
    Io(PathBuf, io::Error),
    Toml(PathBuf, toml::de::Error),
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(path) => write!(
                formatter,
                "no pyproject.toml with [tool.osiris] found from {}",
                path.display()
            ),
            Self::MissingTable(path) => {
                write!(formatter, "{} has no [tool.osiris] table", path.display())
            }
            Self::Io(path, error) => {
                write!(formatter, "could not read {}: {error}", path.display())
            }
            Self::Toml(path, error) => {
                write!(formatter, "invalid TOML in {}: {error}", path.display())
            }
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Deserialize)]
struct PyProject {
    #[serde(default)]
    project: Option<RawProject>,
    tool: Option<ToolTable>,
}

#[derive(Default, Deserialize)]
struct RawProject {
    name: Option<String>,
    version: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

#[derive(Deserialize)]
struct ToolTable {
    osiris: Option<RawConfig>,
}

#[derive(Deserialize)]
#[serde(default, rename_all = "kebab-case")]
struct RawConfig {
    source: Vec<String>,
    target_python: Option<String>,
    strict: bool,
    extensions: Vec<String>,
    build_groups: Vec<String>,
    display_locale: Option<String>,
    trust: RawTrust,
}

impl Default for RawConfig {
    fn default() -> Self {
        Self {
            source: Vec::new(),
            target_python: None,
            strict: true,
            extensions: Vec::new(),
            build_groups: Vec::new(),
            display_locale: None,
            trust: RawTrust::default(),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawTrust {
    contract: Vec<RawTrustContract>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RawTrustContract {
    distribution: String,
    semantic_interface_hash: String,
    ids: Vec<String>,
}
