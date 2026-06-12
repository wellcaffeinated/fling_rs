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
        let socket = format!("/tmp/fling-test-{id}.sock");
        let config_path = format!("/tmp/fling-test-{id}.toml");

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
        std::fs::write(&config_path, config).unwrap();
        let _ = std::fs::remove_file(&socket);

        let process = Command::new(FLING)
            .args(["server", "--socket", &socket, "--config", &config_path])
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

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
    assert!(stderr.contains("not configured"), "unexpected stderr: {stderr}");
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
    assert!(stderr.contains("not permitted"), "unexpected stderr: {stderr}");

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
    assert!(String::from_utf8_lossy(&denied.stderr).contains("not permitted"));

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
