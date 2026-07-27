//! Language server diagnostics log.
//!
//! An LSP server owns stdout for protocol traffic, so log records go to
//! stderr, which editors surface in their language-server output panel. Set
//! `OSIRIS_LSP_LOG_FILE` to additionally append records to a file when the
//! editor does not expose stderr.
//!
//! `OSIRIS_LSP_LOG` selects the level (`off`, `error`, `warn`, `info`,
//! `debug`, `trace`). The stdio transport defaults to `info` so a real session
//! is observable without configuration; embedders and tests stay silent unless
//! they opt in, because the state machine is also used as a library.

use std::{
    fs::OpenOptions,
    io::Write,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU8, Ordering},
    },
    time::Instant,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Level {
    Off = 0,
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl Level {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "0" => Some(Self::Off),
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }
}

/// Level applied when `OSIRIS_LSP_LOG` is unset. Transports raise this.
static DEFAULT_LEVEL: AtomicU8 = AtomicU8::new(Level::Off as u8);

/// Raises the level used when `OSIRIS_LSP_LOG` is unset.
///
/// Call before serving. The environment variable always wins, so a user can
/// still quiet or extend a transport's default.
pub(crate) fn set_default_level(level: Level) {
    DEFAULT_LEVEL.store(level as u8, Ordering::Relaxed);
}

fn configured_level() -> Level {
    static ENVIRONMENT: OnceLock<Option<Level>> = OnceLock::new();
    let configured = ENVIRONMENT.get_or_init(|| {
        std::env::var("OSIRIS_LSP_LOG")
            .ok()
            .as_deref()
            .and_then(Level::parse)
    });
    configured.unwrap_or_else(|| match DEFAULT_LEVEL.load(Ordering::Relaxed) {
        1 => Level::Error,
        2 => Level::Warn,
        3 => Level::Info,
        4 => Level::Debug,
        5 => Level::Trace,
        _ => Level::Off,
    })
}

pub(crate) fn enabled(level: Level) -> bool {
    level <= configured_level()
}

fn started() -> Instant {
    static STARTED: OnceLock<Instant> = OnceLock::new();
    *STARTED.get_or_init(Instant::now)
}

fn file_sink() -> Option<&'static Mutex<std::fs::File>> {
    static SINK: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
    SINK.get_or_init(|| {
        let path = std::env::var_os("OSIRIS_LSP_LOG_FILE")?;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
            .map(Mutex::new)
    })
    .as_ref()
}

/// Records at [`Level::Info`], for callers outside the macros' scope.
pub(crate) fn info(message: &str) {
    if enabled(Level::Info) {
        record(Level::Info, message);
    }
}

/// Records at [`Level::Error`], for callers outside the macros' scope.
pub(crate) fn error(message: &str) {
    if enabled(Level::Error) {
        record(Level::Error, message);
    }
}

/// Writes one record unconditionally. Prefer the [`lsp_log`] macro, or [`info`]
/// and [`error`], so the message is only formatted when the level is enabled.
pub(crate) fn record(level: Level, message: &str) {
    let elapsed = started().elapsed();
    let line = format!(
        "[osr-lsp +{:>8.3}s {:<5}] {message}\n",
        elapsed.as_secs_f64(),
        level.label()
    );
    let _ = std::io::stderr().write_all(line.as_bytes());
    if let Some(sink) = file_sink()
        && let Ok(mut file) = sink.lock()
    {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

macro_rules! lsp_log {
    ($level:expr, $($argument:tt)*) => {
        if crate::lsp::log::enabled($level) {
            crate::lsp::log::record($level, &format!($($argument)*));
        }
    };
}

macro_rules! lsp_error {
    ($($argument:tt)*) => { lsp_log!(crate::lsp::log::Level::Error, $($argument)*) };
}

macro_rules! lsp_info {
    ($($argument:tt)*) => { lsp_log!(crate::lsp::log::Level::Info, $($argument)*) };
}

macro_rules! lsp_debug {
    ($($argument:tt)*) => { lsp_log!(crate::lsp::log::Level::Debug, $($argument)*) };
}
