//! The tool and the resolver, checked against each other.
//!
//! These run the real binary. That is the point: every other test in this
//! workspace exercises a library function, and the thing an operator actually
//! runs is a process with an environment, an exit code and two output streams.
//! A tool that produces a file the resolver will not load is a tool that fails
//! at 3am rather than in CI.
//!
//! The round trip below is also the only proof that `mint-token` is worth
//! having. Before it existed, an operator had to produce
//! `HMAC-SHA256(token_key, "kallisto/token/v1\0" || token)` by hand — which
//! `openssl dgst -hmac` cannot do, because of the label prefix. ADR-0015 D8's
//! token table was unusable without this command; this test says it is not.

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use core_crypto::{SealKey, open};
use naughtian_kallisto::resolver::Snapshot;
use policy_engine::Capability;

const CTL: &str = env!("CARGO_BIN_EXE_kallisto-ctl");

struct Workspace {
    dir: PathBuf,
    seal_key: String,
}

impl Workspace {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("kallisto-ctl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut workspace = Self {
            dir,
            seal_key: String::new(),
        };
        workspace.seal_key = workspace.stdout(&["gen-key"]);
        workspace
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(CTL)
            .args(args)
            .env("KALLISTO_SEAL_KEY", &self.seal_key)
            .output()
            .expect("kallisto-ctl should run")
    }

    fn stdout(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.path(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
        path
    }

    /// Loads a sealed file the way the server does, so the assertion is about
    /// what the resolver will accept rather than about what the tool believes.
    fn load(&self, sealed: &Path) -> Snapshot {
        let bytes = std::fs::read(sealed).unwrap();
        let key = SealKey::from_hex(&self.seal_key).unwrap();
        let opened = open(&bytes, &key, None).unwrap();
        Snapshot::build(opened.view().unwrap(), None).unwrap()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn plaintext(version: u64, token_key: &str, tokens: &str) -> String {
    format!(
        r#"{{
  "version": {version},
  "secrets": {{
    "app/db": {{"username": "admin", "password": "duck-fixture-not-a-credential"}},
    "other/thing": {{"k": "v"}}
  }},
  "policies": {{ "app": [{{"path": "secret/data/app/*", "capabilities": ["read"]}}] }},
  "tokens": {{ {tokens} }},
  "token_key": "{token_key}"
}}"#
    )
}

/// The whole tool, in the order an operator uses it.
#[test]
fn a_token_minted_by_the_tool_authenticates_against_the_resolver() {
    let workspace = Workspace::new("round-trip");
    let token_key = workspace.stdout(&["gen-key"]);

    // 1. Seal a file with an empty token table.
    let plain = workspace.write("plain.json", &plaintext(1, &token_key, ""));
    let sealed = workspace.path("secrets.kal");
    workspace.stdout(&[
        "seal",
        "--in",
        plain.to_str().unwrap(),
        "--out",
        sealed.to_str().unwrap(),
    ]);

    // 2. Mint a token against that file. The token goes to stdout so it can be
    //    piped; the line to paste goes to stderr so it does not pollute it.
    let minted = workspace.run(&[
        "mint-token",
        "--in",
        sealed.to_str().unwrap(),
        "--policy",
        "app",
    ]);
    assert!(minted.status.success());
    let token = String::from_utf8(minted.stdout).unwrap().trim().to_string();
    let instructions = String::from_utf8(minted.stderr).unwrap();

    assert!(token.starts_with("s."), "unexpected token shape: {token}");
    assert!(
        !instructions.contains(&token),
        "the token was echoed into the instructions as well as stdout"
    );

    // 3. Paste the hash in and re-seal, exactly as the instructions say.
    let hash = instructions
        .lines()
        .find_map(|line| line.trim().strip_prefix('"')?.split('"').next())
        .expect("the instructions should carry the hash to paste");
    let plain = workspace.write(
        "plain.json",
        &plaintext(2, &token_key, &format!(r#""{hash}": ["app"]"#)),
    );
    workspace.stdout(&[
        "seal",
        "--in",
        plain.to_str().unwrap(),
        "--out",
        sealed.to_str().unwrap(),
    ]);

    // 4. The resolver accepts the token, and only for what the policy grants.
    let snapshot = workspace.load(&sealed);
    assert!(snapshot.enforces());
    assert_eq!(snapshot.token_count(), 1);
    assert!(snapshot.unknown_policies().is_empty());
    assert!(snapshot.permits(Some(&token), "secret/data/app/db", Capability::Read));
    assert!(!snapshot.permits(Some(&token), "secret/data/other/thing", Capability::Read));
    assert!(!snapshot.permits(
        Some("s.someoneelse"),
        "secret/data/app/db",
        Capability::Read
    ));
}

/// `seal` must not produce a file the resolver would refuse as a rollback —
/// which is what re-sealing at the same version does, and it is silent: the
/// bucket accepts the upload and the sidecars simply keep serving the old one.
#[test]
fn sealing_a_version_the_resolver_would_refuse_is_caught_before_it_is_written() {
    let workspace = Workspace::new("rollback");
    let token_key = workspace.stdout(&["gen-key"]);
    let sealed = workspace.path("secrets.kal");

    let plain = workspace.write("plain.json", &plaintext(7, &token_key, ""));
    workspace.stdout(&[
        "seal",
        "--in",
        plain.to_str().unwrap(),
        "--out",
        sealed.to_str().unwrap(),
    ]);

    let older = workspace.write("older.json", &plaintext(3, &token_key, ""));
    let refused = workspace.run(&[
        "seal",
        "--in",
        older.to_str().unwrap(),
        "--out",
        sealed.to_str().unwrap(),
    ]);
    assert_eq!(refused.status.code(), Some(2));
    let message = String::from_utf8(refused.stderr).unwrap();
    assert!(message.contains("already holds version 7"), "{message}");

    // And the file on disk is untouched, which is the part that matters.
    assert_eq!(workspace.load(&sealed).version, 7);

    // --force is the escape hatch, and it really does overwrite.
    workspace.stdout(&[
        "seal",
        "--in",
        older.to_str().unwrap(),
        "--out",
        sealed.to_str().unwrap(),
        "--force",
    ]);
    assert_eq!(workspace.load(&sealed).version, 3);
}

#[test]
fn bump_version_produces_a_file_the_resolver_still_accepts() {
    let workspace = Workspace::new("bump");
    let token_key = workspace.stdout(&["gen-key"]);
    let plain = workspace.write("plain.json", &plaintext(4, &token_key, ""));
    let sealed = workspace.path("secrets.kal");
    workspace.stdout(&[
        "seal",
        "--in",
        plain.to_str().unwrap(),
        "--out",
        sealed.to_str().unwrap(),
    ]);

    workspace.stdout(&["bump-version", "--in", sealed.to_str().unwrap()]);

    let snapshot = workspace.load(&sealed);
    assert_eq!(snapshot.version, 5);
    // The secrets survived the round trip through decrypt-edit-reseal.
    assert_eq!(snapshot.secret_count(), 2);
    assert_eq!(
        snapshot
            .with_secret("app/db", ToString::to_string)
            .unwrap()
            .unwrap(),
        r#"{"password":"duck-fixture-not-a-credential","username":"admin"}"#
    );
}

/// Neither `verify` nor a failure message may print what is in the file. These
/// are the commands an operator runs while someone is watching a screen share.
#[test]
fn nothing_but_open_ever_prints_a_secret() {
    let workspace = Workspace::new("quiet");
    let token_key = workspace.stdout(&["gen-key"]);
    let plain = workspace.write("plain.json", &plaintext(1, &token_key, ""));
    let sealed = workspace.path("secrets.kal");

    let commands: Vec<Vec<&str>> = vec![
        vec![
            "seal",
            "--in",
            plain.to_str().unwrap(),
            "--out",
            sealed.to_str().unwrap(),
        ],
        vec!["verify", "--in", sealed.to_str().unwrap()],
        vec!["bump-version", "--in", sealed.to_str().unwrap()],
        vec!["open", "--in", sealed.to_str().unwrap()],
    ];

    for command in commands {
        let out = workspace.run(&command);
        let rendered = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        for needle in [
            "duck-fixture-not-a-credential",
            "admin",
            "app/db",
            &token_key,
        ] {
            assert!(
                !rendered.contains(needle),
                "{command:?} printed {needle:?}:\n{rendered}"
            );
        }
    }

    // And with the flag, it prints everything — otherwise the assertion above
    // would pass for a command that simply does nothing.
    let out = workspace.run(&[
        "open",
        "--in",
        sealed.to_str().unwrap(),
        "--yes-print-secrets-to-stdout",
    ]);
    let rendered = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        rendered.contains("duck-fixture-not-a-credential"),
        "{rendered}"
    );
    assert!(rendered.contains("app/db"), "{rendered}");
}

/// A key on the command line is readable by every process on the host. The
/// refusal has to name the environment variable, and must not echo the value.
#[test]
fn a_key_passed_as_an_argument_is_refused_without_being_echoed() {
    let workspace = Workspace::new("no-key-flag");
    let out = workspace.run(&["verify", "--in", "x.kal", "--seal-key", "deadbeefcafe"]);
    assert_eq!(out.status.code(), Some(2));
    let message = String::from_utf8(out.stderr).unwrap();
    assert!(
        message.contains("never a command-line argument"),
        "{message}"
    );
    assert!(
        !message.contains("deadbeefcafe"),
        "the key was echoed: {message}"
    );
}

#[test]
fn a_forged_file_is_refused_and_the_message_says_what_to_check() {
    let workspace = Workspace::new("forged");
    let token_key = workspace.stdout(&["gen-key"]);
    let plain = workspace.write("plain.json", &plaintext(1, &token_key, ""));
    let sealed = workspace.path("secrets.kal");
    workspace.stdout(&[
        "seal",
        "--in",
        plain.to_str().unwrap(),
        "--out",
        sealed.to_str().unwrap(),
    ]);

    let mut bytes = std::fs::read(&sealed).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&sealed, &bytes).unwrap();

    let out = workspace.run(&["verify", "--in", sealed.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let message = String::from_utf8(out.stderr).unwrap();
    assert!(message.contains("failed authentication"), "{message}");
    assert!(
        message.contains("key is wrong or the file has been altered"),
        "{message}"
    );
}

/// `validate` is the promise ADR-0003's Confirmation section made, and the
/// committed example is the file it has to be able to read.
#[test]
fn validate_accepts_the_committed_example_and_rejects_a_broken_one() {
    let workspace = Workspace::new("validate");
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");

    let good = workspace.stdout(&[
        "validate",
        "--config",
        repo.join("kallisto.example.yaml").to_str().unwrap(),
    ]);
    assert!(good.contains("valid"), "{good}");
    assert!(good.contains("127.0.0.1:8200"), "{good}");

    // A non-loopback listener is the ADR's first operational red line, and the
    // point of validating is to hear about it before a deploy, not during one.
    let bad = workspace.write(
        "bad.yaml",
        "apiVersion: kallisto/v1\nkind: Resolver\nspec:\n  listen:\n    address: 0.0.0.0\n  \
         source:\n    type: disk\n    path: /tmp/x.kal\n",
    );
    let out = workspace.run(&["validate", "--config", bad.to_str().unwrap()]);
    assert!(!out.status.success());
    let message = String::from_utf8(out.stderr).unwrap();
    assert!(message.contains("localhost only"), "{message}");
}
