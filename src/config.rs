use std::collections::HashMap;
use std::path::Path;
use anyhow::Result;
use serde::Deserialize;

use crate::glob::glob_match;

#[derive(Deserialize, Clone)]
pub struct Config {
    pub commands: HashMap<String, CommandConfig>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CommandConfig {
    pub executable: String,
    pub working_dir: Option<String>,
    /// Glob patterns matched against the space-joined arguments. The default is
    /// an empty list, which denies every invocation (default-deny).
    #[serde(default)]
    pub allow: Vec<String>,
}

impl CommandConfig {
    /// Returns true if `args` are permitted by this command's allow patterns.
    ///
    /// The arguments are joined with single spaces and matched against each
    /// glob pattern; a match against any one pattern permits the invocation.
    /// With no patterns configured, nothing is permitted.
    pub fn permits(&self, args: &[String]) -> bool {
        let candidate = args.join(" ");
        self.allow.iter().any(|pat| glob_match(pat, &candidate))
    }
}

impl Config {
    /// Authorizes a relayed invocation under the default-deny policy.
    ///
    /// Returns the matching command config, or an error message suitable for
    /// returning to the client when the command is unknown or its arguments are
    /// not permitted by any allow pattern.
    pub fn authorize(&self, cmd: &str, args: &[String]) -> Result<&CommandConfig, String> {
        let entry = self
            .commands
            .get(cmd)
            .ok_or_else(|| format!("command '{cmd}' is not configured"))?;

        if entry.permits(args) {
            Ok(entry)
        } else {
            let invocation = std::iter::once(cmd)
                .chain(args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ");
            Err(format!("'{invocation}' is not permitted by the access rules"))
        }
    }
}

pub fn load(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read config {}: {}", path.display(), e))?;
    toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("failed to parse config {}: {}", path.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let toml = r#"
            [commands.foo]
            executable = "/bin/echo"
            allow = ["list*", "status"]

            [commands.open]
            executable = "/bin/cat"
            allow = ["*"]

            [commands.locked]
            executable = "/bin/echo"
        "#;
        toml::from_str(toml).unwrap()
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn allows_matching_subcommand() {
        let c = config();
        assert!(c.authorize("foo", &argv(&["list"])).is_ok());
        assert!(c.authorize("foo", &argv(&["list", "--all"])).is_ok());
        assert!(c.authorize("foo", &argv(&["status"])).is_ok());
    }

    #[test]
    fn denies_unlisted_subcommand() {
        let c = config();
        assert!(c.authorize("foo", &argv(&["create"])).is_err());
        // `status` is exact-match only; trailing args are not permitted.
        assert!(c.authorize("foo", &argv(&["status", "--force"])).is_err());
    }

    #[test]
    fn denies_unknown_command() {
        let c = config();
        let err = c.authorize("nope", &argv(&["list"])).unwrap_err();
        assert!(err.contains("not configured"), "got: {err}");
    }

    #[test]
    fn wildcard_allows_everything() {
        let c = config();
        assert!(c.authorize("open", &argv(&[])).is_ok());
        assert!(c.authorize("open", &argv(&["whatever", "-x"])).is_ok());
    }

    #[test]
    fn missing_allow_denies_by_default() {
        let c = config();
        assert!(c.authorize("locked", &argv(&[])).is_err());
        assert!(c.authorize("locked", &argv(&["anything"])).is_err());
    }
}
