//! `health`: ask the server on this machine whether it is serving (issue #60).
//!
//! The one command here that opens a socket. It exists because nothing else
//! can probe Kallisto on Kubernetes: the kubelet sends `httpGet` and
//! `tcpSocket` probes to the pod IP and Kallisto binds loopback only
//! (ADR-0015), and the image is distroless, so an `exec` probe has no shell and
//! no HTTP client to run. It does have this binary.
//!
//! It lives in `kallisto-ctl` and not in `kallisto-server` so that a probe
//! every few seconds does not start the server's allocator and process
//! hardening to send one request. What "offline" still means for this tool:
//! nothing here writes over the network, and nothing reads a secret over it.
//! `health` reads a status line from loopback and no more.
//!
//! No HTTP client crate: the request is three lines to an address on this
//! machine, and the answer that matters is the first twelve bytes.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    time::{Duration, Instant},
};

use naughtian_kallisto::config;

use crate::{args::Args, commands::Failure};

/// A kubelet exec probe is killed after `timeoutSeconds`, one second by
/// default. Giving up first means the probe fails with this tool's message
/// instead of the kubelet's.
const DEFAULT_TIMEOUT_MILLIS: u64 = 800;
const HEALTH_REQUEST: &[u8] =
    b"GET /v1/sys/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
/// `HTTP/1.1 200` is twelve bytes. A status line longer than this is not one.
const STATUS_LINE_LIMIT: usize = 256;
const FILE_LOADED: u16 = 200;

/// Exit 0 when the server answers, or with `--ready` only when it holds a
/// file. Exit 1 otherwise, exit 2 when called wrongly.
///
/// ```text
/// kallisto-ctl health            # liveness: any HTTP answer
/// kallisto-ctl health --ready    # readiness and startup: a file is loaded
/// ```
pub fn run_health(args: &Args) -> Result<(), Failure> {
    args.reject_unknown(&[
        "--ready",
        "--config",
        "--listen-address",
        "--listen-port",
        "--timeout-millis",
    ])?;
    let timeout = Duration::from_millis(
        args.number("--timeout-millis")?
            .unwrap_or(DEFAULT_TIMEOUT_MILLIS)
            .max(1),
    );
    let target = probe_target(args, |name| std::env::var(name).ok())?;

    let status = ask_health(target, timeout).map_err(|why| {
        Failure::Failed(format!(
            "no answer from http://{target}/v1/sys/health: {why}"
        ))
    })?;
    judge(status, args.has("--ready"), target)
}

/// The address the server listens on, from the layers the server reads it
/// from: `--listen-*` flags, then `KALLISTO_LISTEN_*`, then the configuration
/// file named by `--config` or `KALLISTO_CONFIG`, then the defaults.
///
/// With no file named, the layers above it still apply. The server refuses to
/// start without one, so a probe in the server's own container always has it.
fn probe_target<F: Fn(&str) -> Option<String>>(
    args: &Args,
    get_env: F,
) -> Result<SocketAddr, Failure> {
    let cli = config::parse_args(listen_flags(args).into_iter()).map_err(usage)?;
    let env = config::from_env(get_env).map_err(usage)?;

    let path = cli.config_path.clone().or_else(|| env.config_path.clone());
    let file = path.as_deref().map(read_spec).transpose()?;
    Ok(config::probe_address(file.as_ref(), env, cli))
}

fn read_spec(path: &Path) -> Result<config::Spec, Failure> {
    config::read_file(path)
        .map(|file| file.spec)
        .map_err(|e| Failure::Failed(e.to_string()))
}

/// The flags this command shares with the server, respelled for the server's
/// own parser so that a bad value gets the server's own message.
fn listen_flags(args: &Args) -> Vec<String> {
    ["--config", "--listen-address", "--listen-port"]
        .iter()
        .filter_map(|flag| args.value(flag).map(|value| format!("{flag}={value}")))
        .collect()
}

fn usage(e: config::ConfigError) -> Failure {
    Failure::Usage(e.to_string())
}

/// One GET, and the status code of the answer.
fn ask_health(target: SocketAddr, timeout: Duration) -> Result<u16, String> {
    let deadline = Instant::now() + timeout;
    let mut stream = TcpStream::connect_timeout(&target, timeout).map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .and_then(|()| stream.write_all(HEALTH_REQUEST))
        .map_err(|e| e.to_string())?;

    let line = read_status_line(&mut stream, deadline)?;
    status_of(&line).ok_or_else(|| {
        format!(
            "expected an HTTP status line such as \"HTTP/1.1 200 OK\", got {:?}",
            String::from_utf8_lossy(&line)
        )
    })
}

/// Reads up to the first line break. The deadline is checked between reads, so
/// a peer that sends a byte at a time cannot hold a probe open.
fn read_status_line(stream: &mut TcpStream, deadline: Instant) -> Result<Vec<u8>, String> {
    let mut line = Vec::with_capacity(STATUS_LINE_LIMIT);
    let mut chunk = [0u8; STATUS_LINE_LIMIT];
    while !line.contains(&b'\n') && line.len() < STATUS_LINE_LIMIT {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("timed out waiting for the status line".to_string());
        }
        stream
            .set_read_timeout(Some(left))
            .map_err(|e| e.to_string())?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => line.extend_from_slice(&chunk[..n]),
            Err(e) => return Err(read_failure(&e)),
        }
    }
    Ok(line)
}

/// A read that hits the socket timeout reports `WouldBlock` or `TimedOut`
/// depending on the platform; both mean the same thing to an operator.
fn read_failure(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
            "timed out waiting for the status line".to_string()
        }
        _ => e.to_string(),
    }
}

/// `HTTP/1.1 503 Service Unavailable` gives 503. Anything that is not an HTTP
/// status line gives `None`.
fn status_of(reply: &[u8]) -> Option<u16> {
    let line = reply
        .split(|byte| *byte == b'\r' || *byte == b'\n')
        .next()?;
    let mut parts = std::str::from_utf8(line).ok()?.split(' ');
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let code = parts.next()?;
    (code.len() == 3).then(|| code.parse().ok()).flatten()
}

/// Liveness is "it answered". Readiness is "it answered 200", which
/// `sys/health` gives only while a file is loaded (`sealed: false`).
fn judge(status: u16, ready: bool, target: SocketAddr) -> Result<(), Failure> {
    if status == FILE_LOADED {
        println!("{target}: serving");
        return Ok(());
    }
    if ready {
        return Err(Failure::Failed(format!(
            "{target} answered HTTP {status}, expected 200: no sealed file is loaded"
        )));
    }
    println!("{target}: answering, HTTP {status}, no sealed file is loaded");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Args {
        Args::parse(list.iter().map(|s| (*s).to_string())).unwrap()
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn target() -> SocketAddr {
        "127.0.0.1:8200".parse().unwrap()
    }

    #[test]
    fn the_status_code_is_read_from_the_first_line_only() {
        assert_eq!(
            status_of(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n"),
            Some(200)
        );
        assert_eq!(
            status_of(b"HTTP/1.0 503 Service Unavailable\r\n"),
            Some(503)
        );
        // No reason phrase, and cut off before the line break.
        assert_eq!(status_of(b"HTTP/1.1 429"), Some(429));
    }

    #[test]
    fn a_reply_that_is_not_http_has_no_status() {
        assert_eq!(status_of(b""), None);
        assert_eq!(status_of(b"SSH-2.0-OpenSSH_9.6\r\n"), None);
        assert_eq!(status_of(b"HTTP/1.1 OK\r\n"), None);
        assert_eq!(status_of(b"HTTP/1.1 2000 OK\r\n"), None);
        assert_eq!(status_of(b"\xff\xfe\r\n"), None);
    }

    #[test]
    fn a_loaded_file_passes_both_probes() {
        judge(200, false, target()).unwrap();
        judge(200, true, target()).unwrap();
    }

    /// The case of issue #60: a server with no file is alive, and not ready.
    #[test]
    fn a_sealed_server_is_alive_and_not_ready() {
        judge(503, false, target()).unwrap();
        let Failure::Failed(message) = judge(503, true, target()).unwrap_err() else {
            panic!("a sealed server is a failed probe, not a usage error");
        };
        assert!(message.contains("HTTP 503"), "{message}");
        assert!(message.contains("127.0.0.1:8200"), "{message}");
    }

    #[test]
    fn with_nothing_set_the_target_is_the_servers_default() {
        assert_eq!(probe_target(&args(&["health"]), no_env).unwrap(), target());
    }

    #[test]
    fn the_target_follows_the_servers_precedence() {
        let env = |name: &str| (name == "KALLISTO_LISTEN_PORT").then(|| "9001".to_string());
        let from_env = probe_target(&args(&["health"]), env).unwrap();
        assert_eq!(from_env.port(), 9001);

        let from_flag = probe_target(&args(&["health", "--listen-port=9002"]), env).unwrap();
        assert_eq!(from_flag.port(), 9002);
    }

    /// The point of reading the server's file: a port changed there needs no
    /// second setting on the probe.
    #[test]
    fn the_target_is_read_from_the_configuration_file() {
        let path =
            std::env::temp_dir().join(format!("kallisto-health-{}.yaml", std::process::id()));
        std::fs::write(
            &path,
            "apiVersion: kallisto/v1\nkind: Resolver\nspec:\n  listen:\n    port: 9003\n  \
             source:\n    type: disk\n    path: /x.kal\n",
        )
        .unwrap();
        let env = |name: &str| (name == "KALLISTO_CONFIG").then(|| path.display().to_string());
        let found = probe_target(&args(&["health"]), env);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(found.unwrap().port(), 9003);
    }

    #[test]
    fn a_bad_port_is_a_usage_error_that_names_the_value() {
        let err = probe_target(&args(&["health", "--listen-port=eighty"]), no_env).unwrap_err();
        let Failure::Usage(message) = err else {
            panic!("expected a usage error");
        };
        assert!(message.contains("eighty"), "{message}");
    }

    #[test]
    fn a_configuration_file_that_is_missing_fails_the_probe() {
        let err = probe_target(&args(&["health", "--config=/nonexistent/k.yaml"]), no_env);
        assert!(matches!(err.unwrap_err(), Failure::Failed(_)));
    }
}
