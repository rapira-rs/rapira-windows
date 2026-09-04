use anyhow::bail;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    #[default]
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Plain,
    Json,
}

#[derive(Debug)]
pub struct LogSettings {
    pub level: LogLevel,
    pub format: LogFormat,
    /// Keys match by prefix. `BTreeMap` gives the filter a stable byte order.
    pub targets: BTreeMap<String, LogLevel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogSection {
    level: Option<LogLevel>,
    format: Option<LogFormat>,
    /// Target keys can contain any module path. Thus, `resolve_log` validates their format because `deny_unknown_fields` cannot validate them.
    #[serde(default)]
    targets: BTreeMap<String, LogLevel>,
}

/// Target names are module paths. A key must have the format that `EnvFilter` parses as a target. The characters `[`, `,`, and `=` are filter syntax.
pub(crate) fn resolve_log(section: LogSection) -> anyhow::Result<LogSettings> {
    for name in section.targets.keys() {
        let mut chars = name.chars();
        let ok = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-'));
        if !ok {
            bail!(
                "log.targets key `{}` is not a log target: use letters, digits and `_` `:` `.` `-`, starting with a letter, digit or `_`",
                name.escape_default()
            );
        }
    }

    Ok(LogSettings {
        level: section.level.unwrap_or_default(),
        format: section.format.unwrap_or_default(),
        targets: section.targets,
    })
}
