//! Builds the child process for a relayed command, optionally wrapping it in a
//! `bwrap` (bubblewrap) sandbox that restricts the command's filesystem view.
//!
//! When a command has a `[commands.<name>.sandbox]` section, the executable is
//! launched inside a fresh mount/user/pid/net namespace where only the runtime
//! libraries and the explicitly bound paths are visible. This is how, e.g., a
//! relayed `cat` can be restricted to a single directory: bind that directory
//! read-only and nothing else (so `/etc/passwd` simply isn't present).

use std::path::Path;

use tokio::process::Command;

use crate::config::{CommandConfig, Sandbox};

/// Directories bound read-only into every sandbox so the executable and its
/// shared libraries resolve. `--ro-bind-try` skips any that don't exist on the
/// host (e.g. `/lib64` on some distros).
const RUNTIME_DIRS: &[&str] = &["/lib", "/lib64", "/bin", "/sbin"];

/// Working directory for a sandboxed command that doesn't configure one. The
/// sandbox always mounts a fresh tmpfs here, so it starts empty, is writable
/// for tools that expect a usable CWD, and is discarded with the process.
const DEFAULT_SANDBOX_CWD: &str = "/tmp";

/// Working directory for a command that has opted out of sandboxing and set no
/// `working_dir`. Relayed commands must never inherit the server's own working
/// directory, which depends on how the admin happened to launch it.
const DEFAULT_UNSANDBOXED_CWD: &str = "/";

/// Build the [`Command`] to spawn for `entry` with `args`.
///
/// Stdio configuration is left to the caller; only the program, arguments,
/// working directory and (if configured) sandbox wrapping are set here.
pub fn build_command(entry: &CommandConfig, args: &[String]) -> Command {
    match &entry.sandbox {
        None => {
            let mut cmd = Command::new(&entry.executable);
            cmd.args(args);
            cmd.current_dir(
                entry.working_dir.as_deref().unwrap_or(DEFAULT_UNSANDBOXED_CWD),
            );
            cmd
        }
        Some(sandbox) => build_sandboxed(entry, sandbox, args),
    }
}

fn build_sandboxed(entry: &CommandConfig, sandbox: &Sandbox, args: &[String]) -> Command {
    let mut cmd = Command::new("bwrap");

    // Minimal runtime image: libraries needed to load the executable, an
    // optional private /proc and /dev, a fresh /tmp, and nothing writable from
    // the host by default.
    cmd.args(["--ro-bind", "/usr", "/usr"]);
    for dir in RUNTIME_DIRS {
        cmd.args(["--ro-bind-try", dir, dir]);
    }
    if sandbox.proc {
        cmd.args(["--proc", "/proc"]);
    }
    if sandbox.dev {
        cmd.args(["--dev", "/dev"]);
    }
    cmd.args(["--tmpfs", "/tmp"]);

    // Isolate namespaces (incl. network), reap on parent death, and detach the
    // controlling terminal to block TIOCSTI-style input injection.
    cmd.arg("--unshare-all");
    cmd.arg("--die-with-parent");
    cmd.arg("--new-session");

    // Explicitly granted paths.
    for path in &sandbox.ro_bind {
        cmd.args(["--ro-bind", path, path]);
    }
    for path in &sandbox.rw_bind {
        cmd.args(["--bind", path, path]);
    }

    match &entry.working_dir {
        // Setting `working_dir` is the admin's explicit grant for that
        // directory: bind it so it exists inside the sandbox, unless an
        // existing bind already covers it (which would otherwise clobber a
        // read-write grant with a read-only one).
        Some(wd) => {
            if !is_covered(wd, sandbox) {
                cmd.args(["--ro-bind", wd, wd]);
            }
            cmd.args(["--chdir", wd]);
        }
        // No working directory configured: start on the sandbox's own tmpfs,
        // which is mounted above and contains nothing from the host.
        None => {
            cmd.args(["--chdir", DEFAULT_SANDBOX_CWD]);
        }
    }

    // End of bwrap options; the relayed command follows.
    cmd.arg("--");
    cmd.arg(&entry.executable);
    cmd.args(args);
    cmd
}

/// True if `path` is already visible inside the sandbox through one of its
/// configured binds, either exactly or as a descendant.
fn is_covered(path: &str, sandbox: &Sandbox) -> bool {
    let path = Path::new(path);
    sandbox
        .ro_bind
        .iter()
        .chain(&sandbox.rw_bind)
        .any(|bound| path.starts_with(Path::new(bound)))
}

/// Whether `bwrap` can be found on `PATH`. Presence only — this doesn't verify
/// that it can actually create namespaces here.
pub fn bwrap_on_path() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join("bwrap").is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use std::ffi::OsStr;

    fn cmd_for(toml: &str, name: &str, args: &[&str]) -> Vec<String> {
        let cfg = config::parse_for_test(toml);
        let entry = cfg.commands.get(name).unwrap();
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let cmd = build_command(entry, &owned);
        let std = cmd.as_std();
        std::iter::once(std.get_program())
            .chain(std.get_args())
            .map(|a: &OsStr| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn sandbox_is_on_by_default() {
        // A command with no `[*.sandbox]` section must still be confined.
        let toml = r#"
            [commands.echo]
            executable = "/bin/echo"
            allow = ["*"]
        "#;
        let argv = cmd_for(toml, "echo", &["hi"]);
        assert_eq!(argv[0], "bwrap", "commands must be sandboxed unless opted out");
    }

    #[test]
    fn sandbox_false_runs_executable_directly() {
        let toml = r#"
            [commands.echo]
            executable = "/bin/echo"
            allow = ["*"]
            sandbox = false
        "#;
        let argv = cmd_for(toml, "echo", &["hi"]);
        assert_eq!(argv, vec!["/bin/echo", "hi"]);
    }

    #[test]
    fn sandbox_wraps_in_bwrap_with_binds() {
        let toml = r#"
            [commands.cat]
            executable = "/bin/cat"
            allow = ["*"]
            [commands.cat.sandbox]
            ro_bind = ["/data/public"]
            rw_bind = ["/data/scratch"]
        "#;
        let argv = cmd_for(toml, "cat", &["/data/public/file"]);
        assert_eq!(argv[0], "bwrap");
        // Grants appear as bind pairs.
        assert!(window_contains(&argv, &["--ro-bind", "/data/public", "/data/public"]));
        assert!(window_contains(&argv, &["--bind", "/data/scratch", "/data/scratch"]));
        // The relayed command follows the `--` separator.
        let sep = argv.iter().position(|a| a == "--").unwrap();
        assert_eq!(&argv[sep + 1..], &["/bin/cat", "/data/public/file"]);
        // Network and other namespaces are isolated.
        assert!(argv.iter().any(|a| a == "--unshare-all"));
    }

    #[test]
    fn proc_and_dev_default_on_but_can_be_disabled() {
        let on = r#"
            [commands.a]
            executable = "/bin/cat"
            allow = ["*"]
            [commands.a.sandbox]
            ro_bind = ["/data"]
        "#;
        let argv = cmd_for(on, "a", &[]);
        assert!(window_contains(&argv, &["--proc", "/proc"]));
        assert!(window_contains(&argv, &["--dev", "/dev"]));

        let off = r#"
            [commands.a]
            executable = "/bin/cat"
            allow = ["*"]
            [commands.a.sandbox]
            ro_bind = ["/data"]
            proc = false
            dev = false
        "#;
        let argv = cmd_for(off, "a", &[]);
        assert!(!window_contains(&argv, &["--proc", "/proc"]));
        assert!(!window_contains(&argv, &["--dev", "/dev"]));
    }

    fn window_contains(haystack: &[String], needle: &[&str]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }
}
