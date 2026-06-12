use std::collections::HashMap;
use std::path::Path;
use anyhow::{anyhow, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

/// Runtime configuration: command name → resolved command config.
#[derive(Clone)]
pub struct Config {
    pub commands: HashMap<String, CommandConfig>,
}

/// A single relayable command with its compiled access rules.
#[derive(Clone, Debug)]
pub struct CommandConfig {
    pub executable: String,
    pub working_dir: Option<String>,
    /// Compiled allow globs, matched against the space-joined arguments.
    allow: GlobSet,
    /// Optional bubblewrap sandbox restricting the command's filesystem view.
    pub sandbox: Option<Sandbox>,
}

/// Filesystem sandbox applied to a command via `bwrap` (bubblewrap).
#[derive(Clone, Debug, Deserialize)]
pub struct Sandbox {
    /// Paths bind-mounted read-only into the sandbox.
    #[serde(default)]
    pub ro_bind: Vec<String>,
    /// Paths bind-mounted read-write into the sandbox.
    #[serde(default)]
    pub rw_bind: Vec<String>,
    /// Mount a fresh private `/proc` (default true). Disable for commands that
    /// don't need it; mounting a fresh procfs requires privileges that some
    /// nested/locked-down environments (e.g. Docker's masked `/proc`) withhold.
    #[serde(default = "default_true")]
    pub proc: bool,
    /// Mount a minimal `/dev` with the standard device nodes (default true).
    #[serde(default = "default_true")]
    pub dev: bool,
}

fn default_true() -> bool {
    true
}

impl CommandConfig {
    /// Returns true if `args` are permitted by this command's allow patterns.
    ///
    /// The arguments are joined with single spaces and matched against the
    /// compiled glob set; a match against any one pattern permits the
    /// invocation. With no patterns configured, nothing is permitted.
    pub fn permits(&self, args: &[String]) -> bool {
        self.allow.is_match(args.join(" "))
    }
}

impl Config {
    /// Authorizes a relayed invocation under the default-deny policy.
    ///
    /// Returns the matching command config, or an error message (for
    /// server-side logging) when the command is unknown or its arguments are
    /// not permitted by any allow pattern. The message returned to the *client*
    /// is deliberately uniform — see `server.rs`.
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
            Err(format!("arguments not permitted by access rules: '{invocation}'"))
        }
    }
}

// ---------------------------------------------------------------------------
// Deserialization: raw TOML shapes compiled into the runtime `Config`.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawConfig {
    commands: HashMap<String, RawCommand>,
}

#[derive(Deserialize)]
struct RawCommand {
    executable: String,
    working_dir: Option<String>,
    /// Glob patterns matched against the space-joined arguments. The default is
    /// an empty list, which denies every invocation (default-deny).
    #[serde(default)]
    allow: Vec<String>,
    sandbox: Option<Sandbox>,
}

pub fn load(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("failed to read config {}: {}", path.display(), e))?;
    parse(&content).map_err(|e| anyhow!("failed to parse config {}: {}", path.display(), e))
}

/// Parses config text and compiles each command's allow globs, validating the
/// patterns at startup so a bad glob fails fast rather than at request time.
fn parse(content: &str) -> Result<Config> {
    let raw: RawConfig = toml::from_str(content)?;

    let mut commands = HashMap::with_capacity(raw.commands.len());
    for (name, rc) in raw.commands {
        let mut builder = GlobSetBuilder::new();
        for pat in &rc.allow {
            let glob = Glob::new(pat)
                .map_err(|e| anyhow!("command '{name}': invalid allow glob '{pat}': {e}"))?;
            builder.add(glob);
        }
        let allow = builder
            .build()
            .map_err(|e| anyhow!("command '{name}': {e}"))?;

        commands.insert(
            name,
            CommandConfig {
                executable: rc.executable,
                working_dir: rc.working_dir,
                allow,
                sandbox: rc.sandbox,
            },
        );
    }

    Ok(Config { commands })
}

/// Parse config text, panicking on error. For use by tests in sibling modules.
#[cfg(test)]
pub fn parse_for_test(content: &str) -> Config {
    parse(content).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let toml = r#"
            [commands.foo]
            executable = "/bin/echo"
            allow = ["list*", "status", "get {a,b}/config"]

            [commands.open]
            executable = "/bin/cat"
            allow = ["*"]

            [commands.locked]
            executable = "/bin/echo"
        "#;
        parse(toml).unwrap()
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
    fn supports_real_glob_syntax() {
        // Brace alternation comes from globset, not a hand-rolled matcher.
        let c = config();
        assert!(c.authorize("foo", &argv(&["get", "a/config"])).is_ok());
        assert!(c.authorize("foo", &argv(&["get", "b/config"])).is_ok());
        assert!(c.authorize("foo", &argv(&["get", "c/config"])).is_err());
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
    fn wildcard_allows_everything_including_no_args() {
        let c = config();
        assert!(c.authorize("open", &argv(&[])).is_ok());
        assert!(c.authorize("open", &argv(&["whatever", "-x"])).is_ok());
        assert!(c.authorize("open", &argv(&["/some/path"])).is_ok());
    }

    #[test]
    fn missing_allow_denies_by_default() {
        let c = config();
        assert!(c.authorize("locked", &argv(&[])).is_err());
        assert!(c.authorize("locked", &argv(&["anything"])).is_err());
    }

    #[test]
    fn invalid_glob_is_rejected_at_load() {
        let toml = r#"
            [commands.bad]
            executable = "/bin/echo"
            allow = ["["]
        "#;
        assert!(parse(toml).is_err());
    }
}
