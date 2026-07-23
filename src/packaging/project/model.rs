#[derive(Debug)]
pub enum ConfigError {
    NotFound(PathBuf),
    MissingConfig(PathBuf),
    Io(PathBuf, io::Error),
    Toml(PathBuf, toml::de::Error),
    Jsonc(PathBuf, json5::Error),
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(path) => write!(
                formatter,
                "no osiris.jsonc project found from {}",
                path.display()
            ),
            Self::MissingConfig(path) => {
                write!(formatter, "{} was not found", path.display())
            }
            Self::Io(path, error) => {
                write!(formatter, "could not read {}: {error}", path.display())
            }
            Self::Toml(path, error) => {
                write!(formatter, "invalid TOML in {}: {error}", path.display())
            }
            Self::Jsonc(path, error) => {
                write!(formatter, "invalid JSONC in {}: {error}", path.display())
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
}

#[derive(Default, Deserialize)]
struct RawProject {
    name: Option<String>,
    version: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}
