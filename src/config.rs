use std::collections::HashMap;
use std::path::Path;
use anyhow::Result;
use serde::Deserialize;

#[derive(Deserialize, Clone)]
pub struct Config {
    pub commands: HashMap<String, CommandConfig>,
}

#[derive(Deserialize, Clone)]
pub struct CommandConfig {
    pub executable: String,
    pub working_dir: Option<String>,
}

pub fn load(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read config {}: {}", path.display(), e))?;
    toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("failed to parse config {}: {}", path.display(), e))
}
