//! `kallisto-ctl health`, as a kubelet sees it: a process and its exit code.
//!
//! Issue #60 asks for the exit codes to be tested against a fake server rather
//! than a real one. A real `kallisto-server` would need a seal key, a sealed
//! file and a free port to say "200" or "503"; what the probe has to get right
//! is what it does with each answer, and with no answer at all.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

const CTL: &str = env!("CARGO_BIN_EXE_kallisto-ctl");

const SERVING: &str = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
const SEALED: &str =
    "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";

/// Stands in for `kallisto-server` on a loopback port of its own.
struct FakeHealthServer {
    address: SocketAddr,
}

impl FakeHealthServer {
    /// Answers every connection with `reply`, whatever was asked.
    fn answering(reply: &'static str) -> Self {
        Self::serving_with(move |mut stream| {
            let mut request = [0u8; 512];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(reply.as_bytes());
        })
    }

    /// Accepts the connection and never answers, like a server whose workers
    /// are all stuck.
    fn silent() -> Self {
        Self::serving_with(|stream| {
            thread::sleep(Duration::from_secs(30));
            drop(stream);
        })
    }

    fn serving_with(respond: impl Fn(std::net::TcpStream) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let respond = std::sync::Arc::new(respond);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let respond = std::sync::Arc::clone(&respond);
                thread::spawn(move || respond(stream));
            }
        });
        Self { address }
    }

    /// A loopback port with nothing behind it: bound once, then released.
    fn gone() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        Self { address }
    }

    fn probe(&self, extra: &[&str]) -> Output {
        Command::new(CTL)
            .arg("health")
            .args(extra)
            // Through the environment, the way a container's probe gets it.
            .env("KALLISTO_LISTEN_PORT", self.address.port().to_string())
            .env_remove("KALLISTO_CONFIG")
            .env_remove("KALLISTO_LISTEN_ADDRESS")
            .output()
            .expect("kallisto-ctl should run")
    }
}

fn exit_code(out: &Output) -> i32 {
    out.status
        .code()
        .expect("kallisto-ctl was killed by a signal")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_server_with_a_file_passes_liveness_and_readiness() {
    let server = FakeHealthServer::answering(SERVING);
    assert_eq!(exit_code(&server.probe(&[])), 0);
    assert_eq!(exit_code(&server.probe(&["--ready"])), 0);
}

/// The pod of issue #60: started while the bucket was down, nothing to serve.
/// It must stay alive, so the kubelet does not restart it in a loop, and it
/// must not be ready, so no Service sends it traffic.
#[test]
fn a_sealed_server_passes_liveness_and_fails_readiness() {
    let server = FakeHealthServer::answering(SEALED);
    assert_eq!(exit_code(&server.probe(&[])), 0);

    let ready = server.probe(&["--ready"]);
    assert_eq!(exit_code(&ready), 1, "{}", stderr(&ready));
    assert!(stderr(&ready).contains("HTTP 503"), "{}", stderr(&ready));
}

#[test]
fn no_server_fails_both_probes() {
    let server = FakeHealthServer::gone();
    for flags in [&[][..], &["--ready"][..]] {
        let out = server.probe(flags);
        assert_eq!(exit_code(&out), 1, "{}", stderr(&out));
        assert!(stderr(&out).contains("no answer"), "{}", stderr(&out));
    }
}

/// A liveness probe exists for this case. It has to fail, and it has to fail
/// by its own clock: the kubelet kills an exec probe at `timeoutSeconds`, and a
/// probe that is killed reports nothing.
#[test]
fn a_server_that_accepts_and_never_answers_fails_within_the_timeout() {
    let server = FakeHealthServer::silent();
    let started = Instant::now();
    let out = server.probe(&["--timeout-millis=200"]);

    assert_eq!(exit_code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("timed out"), "{}", stderr(&out));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the probe took {:?} with a 200 ms timeout",
        started.elapsed()
    );
}

/// Something else on the port is not a live Kallisto.
#[test]
fn an_answer_that_is_not_http_fails_both_probes() {
    let server = FakeHealthServer::answering("SSH-2.0-OpenSSH_9.6\r\n");
    let out = server.probe(&[]);
    assert_eq!(exit_code(&out), 1, "{}", stderr(&out));
    assert!(stderr(&out).contains("SSH-2.0"), "{}", stderr(&out));
}

/// Exit 2 is "the probe is written wrongly", which must not look like
/// "the server is down" in `kubectl describe`.
#[test]
fn a_misspelt_flag_is_a_usage_error_not_a_failed_probe() {
    let server = FakeHealthServer::answering(SERVING);
    let out = server.probe(&["--redy"]);
    assert_eq!(exit_code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("--redy"), "{}", stderr(&out));
}

/// A flag beats the environment, as it does for the server.
#[test]
fn the_listen_port_flag_overrides_the_environment() {
    let serving = FakeHealthServer::answering(SERVING);
    let gone = FakeHealthServer::gone();
    let flag = format!("--listen-port={}", serving.address.port());
    assert_eq!(exit_code(&gone.probe(&[&flag])), 0);
}
