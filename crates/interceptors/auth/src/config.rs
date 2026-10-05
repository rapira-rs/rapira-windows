use std::path::{Path, PathBuf};

use anyhow::bail;
use rapira_config::opt_path;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub tokens_file: Option<String>,
}

#[derive(Debug)]
pub struct Settings {
    pub tokens_file: PathBuf,
}

pub fn resolve(section: Section, dir: &Path) -> anyhow::Result<Settings> {
    let Some(tokens_file) = opt_path(dir, section.tokens_file.as_deref())? else {
        bail!("grpc.auth.tokens_file is required");
    };
    Ok(Settings { tokens_file })
}
