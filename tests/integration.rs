use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const FLING: &str = env!("CARGO_BIN_EXE_fling");

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

struct TestServer {
    process: std::process::Child,
    pub socket: String,
}

/// Overrides for how the server process itself is launched.
#[derive(Default)]
struct ServerOpts {
    /// Socket path, when the default `/tmp/fling-test-{id}.sock` won't do.
    socket: Option<String>,
    /// Working directory of the *server* process, which relayed commands
    /// currently inherit.
    cwd: Option<String>,
    /// Environment overrides (e.g. emptying PATH to hide `bwrap`).
    env: Vec<(String, String)>,
    /// Cap the server's open file descriptors, to reach exhaustion cheaply.
    fd_limit: Option<u32>,
}

impl TestServer {
    /// Start a server allowing every argument for each command (`allow = ["*"]`).
    fn start(id: &str, commands: &[(&str, &str)]) -> Self {
        let with_rules: Vec<_> = commands
            .iter()
            .map(|(name, exe)| (*name, *exe, &["*"][..]))
            .collect();
        Self::start_with_rules(id, &with_rules)
    }

    /// Start a server with explicit allow globs per command.
    fn start_with_rules(id: &str, commands: &[(&str, &str, &[&str])]) -> Self {
        let mut config = String::new();
        for (name, exe, allow) in commands {
            let patterns = allow
                .iter()
                .map(|p| format!("\"{p}\""))
                .collect::<Vec<_>>()
                .join(", ");
            config.push_str(&format!(
                "[commands.{name}]\nexecutable = \"{exe}\"\nallow = [{patterns}]\n\n"
            ));
        }
        Self::start_with_config(id, &config, ServerOpts::default())
    }

    /// Start a server from verbatim config text, for shapes the helpers above
    /// can't express (sandbox tables, `sandbox = false`, top-level settings).
    fn start_with_config(id: &str, config: &str, opts: ServerOpts) -> Self {
        let socket = opts
            .socket
            .clone()
            .unwrap_or_else(|| format!("/tmp/fling-test-{id}.sock"));
        let config_path = format!("/tmp/fling-test-{id}.toml");

        std::fs::write(&config_path, config).unwrap();
        let _ = std::fs::remove_file(&socket);

        // `ulimit` is a shell builtin, so an fd cap means going through sh.
        let mut command = match opts.fd_limit {
            None => {
                let mut c = Command::new(FLING);
                c.args(["server", "--socket", &socket, "--config", &config_path]);
                c
            }
            Some(limit) => {
                let mut c = Command::new("sh");
                c.arg("-c").arg(format!(
                    "ulimit -n {limit}; exec {FLING} server --socket {socket} --config {config_path}"
                ));
                c
            }
        };
        command.stderr(Stdio::null());
        if let Some(cwd) = &opts.cwd {
            command.current_dir(cwd);
        }
        for (key, value) in &opts.env {
            command.env(key, value);
        }
        let process = command.spawn().unwrap();

        // Wait up to 2s for the server to be accepting connections.
        // Checking file existence alone isn't enough under parallel test load;
        // we try an actual connect so we know accept() is running.
        let mut ready = false;
        for _ in 0..40 {
            thread::sleep(Duration::from_millis(50));
            if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
                ready = true;
                break;
            }
        }
        assert!(ready, "server never became ready for test '{id}'");

        TestServer { process, socket }
    }

    /// True while the server process is still running.
    fn is_alive(&mut self) -> bool {
        matches!(self.process.try_wait(), Ok(None))
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(FLING)
            .arg("--socket")
            .arg(&self.socket)
            .args(args)
            .output()
            .unwrap()
    }

    fn run_with_stdin(&self, args: &[&str], stdin_data: &[u8]) -> std::process::Output {
        let mut child = Command::new(FLING)
            .arg("--socket")
            .arg(&self.socket)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin_data).unwrap();
        child.wait_with_output().unwrap()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.process.kill().ok();
        self.process.wait().ok();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn echo_basic() {
    let s = TestServer::start("echo-basic", &[("echo", "/bin/echo")]);
    let out = s.run(&["echo", "hello", "world"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "hello world\n");
}

#[test]
fn stdin_forwarded_through_cat() {
    let s = TestServer::start("stdin-cat", &[("cat", "/bin/cat")]);
    let out = s.run_with_stdin(&["cat"], b"hello from stdin\n");
    assert!(out.status.success());
    assert_eq!(out.stdout, b"hello from stdin\n");
}

#[test]
fn disallowed_command_rejected() {
    let s = TestServer::start("disallowed", &[("echo", "/bin/echo")]);
    let out = s.run(&["sneaky"]);
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    // Unknown commands and disallowed args share one uniform denial message.
    assert_eq!(stderr, "You are not authorized to execute this command\n");
}

#[test]
fn glob_rules_allow_and_deny_subcommands() {
    // `foo list ...` is permitted; `foo create ...` is not.
    let s = TestServer::start_with_rules(
        "glob-rules",
        &[("foo", "/bin/echo", &["list*", "status"])],
    );

    let allowed = s.run(&["foo", "list", "everything"]);
    assert!(allowed.status.success());
    assert_eq!(String::from_utf8(allowed.stdout).unwrap(), "list everything\n");

    let denied = s.run(&["foo", "create", "thing"]);
    assert_eq!(denied.status.code(), Some(1));
    let stderr = String::from_utf8(denied.stderr).unwrap();
    assert_eq!(stderr, "You are not authorized to execute this command\n");

    // Exact-match pattern: `status` alone is allowed, with extra args is not.
    assert!(s.run(&["foo", "status"]).status.success());
    assert_eq!(s.run(&["foo", "status", "--force"]).status.code(), Some(1));
}

#[test]
fn symlink_invocation_relays_by_name() {
    // The binary, symlinked as `greet`, should relay the command `greet`,
    // taking the socket from FLING_SOCKET and forwarding all args verbatim.
    let s = TestServer::start_with_rules("symlink", &[("greet", "/bin/echo", &["*"])]);

    let link_dir = "/tmp/fling-test-symlink-bin".to_string();
    let _ = std::fs::remove_dir_all(&link_dir);
    std::fs::create_dir_all(&link_dir).unwrap();
    let link = format!("{link_dir}/greet");
    std::os::unix::fs::symlink(FLING, &link).unwrap();

    let out = Command::new(&link)
        .args(["hello", "world"])
        .env("FLING_SOCKET", format!("unix:{}", s.socket))
        .output()
        .unwrap();

    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "hello world\n");

    let _ = std::fs::remove_dir_all(&link_dir);
}

#[test]
fn symlink_invocation_respects_access_rules() {
    // A symlinked command is still subject to the server's glob rules.
    let s = TestServer::start_with_rules("symlink-deny", &[("tool", "/bin/echo", &["list*"])]);

    let link_dir = "/tmp/fling-test-symlink-deny-bin".to_string();
    let _ = std::fs::remove_dir_all(&link_dir);
    std::fs::create_dir_all(&link_dir).unwrap();
    let link = format!("{link_dir}/tool");
    std::os::unix::fs::symlink(FLING, &link).unwrap();

    let denied = Command::new(&link)
        .args(["delete", "everything"])
        .env("FLING_SOCKET", format!("unix:{}", s.socket))
        .output()
        .unwrap();
    assert_eq!(denied.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&denied.stderr),
        "You are not authorized to execute this command\n"
    );

    let _ = std::fs::remove_dir_all(&link_dir);
}

#[test]
fn exit_code_propagated() {
    let s = TestServer::start("exit-code", &[("false", "/bin/false"), ("true", "/bin/true")]);
    assert_eq!(s.run(&["false"]).status.code(), Some(1));
    assert_eq!(s.run(&["true"]).status.code(), Some(0));
}

#[test]
fn stderr_routed_separately() {
    let s = TestServer::start("stderr", &[("sh", "/bin/sh")]);
    let out = s.run(&["sh", "-c", "echo stdout-line; echo stderr-line >&2"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "stdout-line\n");
    assert_eq!(String::from_utf8(out.stderr).unwrap(), "stderr-line\n");
}

#[test]
fn args_with_hyphens_and_spaces() {
    let s = TestServer::start("args-hyphens", &[("echo", "/bin/echo")]);
    let out = s.run(&["echo", "--", "--flag", "hello world", "-n"]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("--flag"));
    assert!(stdout.contains("hello world"));
}

#[test]
fn large_output_byte_perfect() {
    let s = TestServer::start("large-output", &[("cat", "/bin/cat")]);
    let data: Vec<u8> = (0u8..=255).cycle().take(1_000_000).collect();
    let out = s.run_with_stdin(&["cat"], &data);
    assert!(out.status.success());
    assert_eq!(out.stdout.len(), 1_000_000, "byte count mismatch");
    assert_eq!(out.stdout, data, "binary content mismatch");
}

#[test]
fn binary_round_trip() {
    let s = TestServer::start("binary", &[("cat", "/bin/cat")]);
    // All 256 byte values in sequence
    let data: Vec<u8> = (0u8..=255).collect();
    let out = s.run_with_stdin(&["cat"], &data);
    assert!(out.status.success());
    assert_eq!(out.stdout, data);
}

#[test]
fn concurrent_clients_isolated() {
    let s = TestServer::start("concurrent", &[("cat", "/bin/cat")]);
    let socket = s.socket.clone();

    let handles: Vec<_> = (0u32..10)
        .map(|i| {
            let socket = socket.clone();
            thread::spawn(move || {
                let input = format!("client {i}\n");
                let mut child = Command::new(FLING)
                    .arg("--socket")
                    .arg(&socket)
                    .arg("cat")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(input.as_bytes())
                    .unwrap();
                let out = child.wait_with_output().unwrap();
                (i, input, out)
            })
        })
        .collect();

    for h in handles {
        let (i, expected, out) = h.join().unwrap();
        assert!(out.status.success(), "client {i} failed");
        assert_eq!(
            out.stdout,
            expected.as_bytes(),
            "client {i} got wrong output"
        );
    }
}

#[test]
fn empty_stdin() {
    let s = TestServer::start("empty-stdin", &[("cat", "/bin/cat")]);
    let out = s.run_with_stdin(&["cat"], b"");
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

// ---------------------------------------------------------------------------
// Defaults: confinement is on unless explicitly disabled
// ---------------------------------------------------------------------------

/// Creates an empty scratch directory containing `marker.txt`, returning its
/// path. Used to prove what a relayed command can and cannot reach.
fn scratch_with_marker(id: &str) -> String {
    let dir = format!("/tmp/fling-test-{id}.d");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(format!("{dir}/marker.txt"), "marker contents\n").unwrap();
    dir
}

#[test]
fn command_without_sandbox_section_cannot_read_host_files() {
    // The headline default: no `[*.sandbox]` section must still mean confined.
    // `allow = ["*"]` permits the argument; the filesystem must not.
    let config = r#"
        [commands.cat]
        executable = "/bin/cat"
        allow = ["*"]
    "#;
    let s = TestServer::start_with_config("default-confined", config, ServerOpts::default());

    let out = s.run(&["cat", "/etc/passwd"]);
    assert!(
        !out.status.success(),
        "unsandboxed-by-default command read /etc/passwd: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("root:"),
        "leaked /etc/passwd contents"
    );
}

#[test]
fn default_working_dir_reaches_nothing() {
    // With no `working_dir`, a relayed command must not inherit the server's
    // CWD — relative paths should resolve into an empty directory.
    let dir = scratch_with_marker("default-cwd");
    let config = r#"
        [commands.cat]
        executable = "/bin/cat"
        allow = ["*"]
    "#;
    let s = TestServer::start_with_config(
        "default-cwd",
        config,
        ServerOpts { cwd: Some(dir.clone()), ..Default::default() },
    );

    let out = s.run(&["cat", "marker.txt"]);
    assert!(
        !out.status.success(),
        "relayed command inherited the server's CWD and read {dir}/marker.txt"
    );
}

#[test]
fn missing_bwrap_fails_closed() {
    // If `bwrap` can't be found, a sandboxed command must fail — never fall
    // back to running unconfined.
    let config = r#"
        [commands.cat]
        executable = "/bin/cat"
        allow = ["*"]

        [commands.cat.sandbox]
        ro_bind = []
    "#;
    let s = TestServer::start_with_config(
        "no-bwrap",
        config,
        ServerOpts {
            env: vec![("PATH".to_string(), String::new())],
            ..Default::default()
        },
    );

    let out = s.run(&["cat", "/etc/passwd"]);
    assert!(!out.status.success(), "sandboxed command ran without bwrap");
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("root:"),
        "fell back to running unconfined"
    );
}

#[test]
fn sandbox_false_disables_confinement() {
    // The documented escape hatch has to actually work.
    let config = r#"
        [commands.cat]
        executable = "/bin/cat"
        allow = ["*"]
        sandbox = false
    "#;
    let s = TestServer::start_with_config("sandbox-off", config, ServerOpts::default());

    let out = s.run(&["cat", "/etc/passwd"]);
    assert!(
        out.status.success(),
        "sandbox = false should run unconfined: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("root:"));
}

#[test]
fn working_dir_grants_access_to_itself() {
    // Setting `working_dir` is the admin's explicit grant: the directory must
    // be readable from inside the sandbox, and become the CWD.
    let dir = scratch_with_marker("wd-grant");
    // The sandbox section is explicit so this exercises the bind path even
    // before sandboxing becomes the default; note it grants no paths itself —
    // `working_dir` alone must be enough.
    let config = format!(
        r#"
        [commands.cat]
        executable = "/bin/cat"
        allow = ["*"]
        working_dir = "{dir}"

        [commands.cat.sandbox]
        ro_bind = []
        "#
    );
    let s = TestServer::start_with_config("wd-grant", &config, ServerOpts::default());

    let out = s.run(&["cat", "marker.txt"]);
    assert!(
        out.status.success(),
        "working_dir should be bound into the sandbox: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "marker contents\n");
}

#[test]
fn survives_connection_flood() {
    // Exhausting the server's file descriptors must be transient: accept()
    // failures are not a reason to take the whole relay down. The fd cap only
    // makes exhaustion cheap to reach — under systemd's default LimitNOFILE
    // this is ~1024 idle connections from any local user.
    let config = r#"
        [commands.echo]
        executable = "/bin/echo"
        allow = ["*"]
    "#;
    let mut s = TestServer::start_with_config(
        "flood",
        config,
        ServerOpts { fd_limit: Some(64), ..Default::default() },
    );

    // Hold connections open without ever completing a handshake.
    let mut held = Vec::new();
    for _ in 0..200 {
        match std::os::unix::net::UnixStream::connect(&s.socket) {
            Ok(c) => held.push(c),
            Err(_) => break, // server refusing new connections is fine
        }
    }
    thread::sleep(Duration::from_millis(300));
    assert!(s.is_alive(), "server died under a connection flood");

    // Once the flood lets go, service must resume.
    drop(held);
    thread::sleep(Duration::from_millis(300));
    let out = s.run(&["echo", "recovered"]);
    assert!(
        out.status.success(),
        "server stopped serving after the flood: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "recovered\n");
}

#[test]
fn socket_parent_directory_is_created() {
    // The default socket lives in /run/fling/, a directory that won't exist on
    // a fresh install; the server should create it rather than fail to bind.
    let base = "/tmp/fling-test-sockdir.d";
    let _ = std::fs::remove_dir_all(base);
    let socket = format!("{base}/nested/fling.sock");

    let config = r#"
        [commands.echo]
        executable = "/bin/echo"
        allow = ["*"]
    "#;
    let s = TestServer::start_with_config(
        "sockdir",
        config,
        ServerOpts { socket: Some(socket.clone()), ..Default::default() },
    );

    let out = s.run(&["echo", "hi"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "hi\n");

    let _ = std::fs::remove_dir_all(base);
}
