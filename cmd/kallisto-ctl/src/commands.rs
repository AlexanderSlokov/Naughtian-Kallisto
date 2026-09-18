//! The commands themselves.
//!
//! One property holds across all of them: **no key is ever a command-line
//! argument.** Arguments are world-readable through `ps` and they land in shell
//! history and CI logs; keys come from the environment, and `args.rs` refuses
//! the flag by name so the mistake is loud rather than silent.
//!
//! A second, less obvious one: this tool *does* hold cleartext, and that is not
//! a contradiction of ADR-0015 D13. D13 is about the long-running resolver —
//! the process that serves secrets for weeks and is worth dumping the memory
//! of. This is a short-lived command an operator runs on a machine where the
//! plaintext file is already sitting on disk next to it.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use core_crypto::{Contents, SealError, SealKey, open, peek_version, seal};
use naughtian_kallisto::config::{self, Config, SEAL_KEY_ENV};
use policy_engine::TokenKey;

use crate::args::{ArgError, Args};

pub const TOKEN_KEY_ENV: &str = "KALLISTO_TOKEN_KEY";

#[derive(Debug)]
pub enum Failure {
    /// The operator asked for something impossible. Exit code 2.
    Usage(String),
    /// The operator asked for something reasonable and it did not work.
    Failed(String),
}

impl From<ArgError> for Failure {
    fn from(e: ArgError) -> Self {
        Self::Usage(e.0)
    }
}

fn seal_key() -> Result<SealKey, Failure> {
    let hex = std::env::var(SEAL_KEY_ENV).map_err(|_| {
        Failure::Usage(format!(
            "{SEAL_KEY_ENV} is not set. It holds the 32-byte seal key, hex-encoded; \
             `kallisto-ctl gen-key` will produce one."
        ))
    })?;
    SealKey::from_hex(&hex)
        .map_err(|e| Failure::Usage(format!("{SEAL_KEY_ENV} is not usable: {e}")))
}

fn read(path: &str) -> Result<Vec<u8>, Failure> {
    fs::read(path).map_err(|e| Failure::Failed(format!("cannot read {path}: {e}")))
}

/// Write to a sibling temporary file and rename over the target.
///
/// A half-written sealed file is a file the resolver refuses, and if that is
/// the only copy on the machine the sidecar comes up sealed. Rename is atomic
/// within a filesystem, so a reader sees the old file or the new one.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    let temporary = temporary_beside(path);
    let write = || -> std::io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&temporary);
        Failure::Failed(format!("cannot write {}: {e}", path.display()))
    })
}

fn temporary_beside(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp.{}", std::process::id()));
    path.with_file_name(name)
}

/// Parses the plaintext without letting `serde_json` say what it saw.
///
/// Its message quotes the offending value, and in this file the offending value
/// is a secret. The position survives, because that is what an operator
/// actually needs, and so does a reminder of the shape — the format uses
/// `token_key`, not the `tokenKey` that the *configuration* file's camelCase
/// would lead you to type, and `deny_unknown_fields` means that difference is
/// a hard error rather than a field quietly ignored.
fn parse_plaintext(text: &str, path: &str) -> Result<Contents, Failure> {
    serde_json::from_str::<Contents>(text).map_err(|e| {
        Failure::Failed(format!(
            "{path} is not a valid plaintext secrets file (line {}, column {}).\n\
             Expected: {{\"version\": N, \"secrets\": {{..}}, \"policies\": {{..}}, \
             \"tokens\": {{..}}, \"token_key\": \"..\"}} — note token_key, not tokenKey, \
             and unknown fields are refused rather than ignored.\n\
             (The parser's own message is withheld: it quotes the value it choked on, and \
             in this file that is a secret.)",
            e.line(),
            e.column()
        ))
    })
}

// -----------------------------------------------------------------------------

pub fn run_seal(args: &Args) -> Result<(), Failure> {
    args.reject_unknown(&["--in", "--out", "--version", "--force"])?;
    let input = args.required("--in")?;
    let output = args.required("--out")?;
    let key = seal_key()?;

    let text = fs::read_to_string(input)
        .map_err(|e| Failure::Failed(format!("cannot read {input}: {e}")))?;
    let mut contents = parse_plaintext(&text, input)?;
    if let Some(version) = args.number("--version")? {
        contents.version = version;
    }

    // The resolver refuses a file older than the one it holds (ADR-0015 D2,
    // QĐ-2), so sealing a lower version over a higher one produces a file that
    // silently stops being picked up. Catching it here costs one `peek_version`
    // and no key material.
    if let Ok(existing) = fs::read(output)
        && let Ok(held) = peek_version(&existing)
        && held >= contents.version
        && !args.has("--force")
    {
        return Err(Failure::Usage(format!(
            "{output} already holds version {held}, and this would write version {}. \
             The resolver would refuse the result. Raise --version, or pass --force if \
             you are rebuilding from scratch.",
            contents.version
        )));
    }

    validate_token_table(&contents)?;

    let bytes = seal(&contents, &key).map_err(|e| Failure::Failed(format!("cannot seal: {e}")))?;
    write_atomically(Path::new(output), &bytes)?;

    println!(
        "sealed version {} into {output}: {} secret(s), {} polic(ies), {} token(s)",
        contents.version,
        contents.secrets.len(),
        contents.policies.len(),
        contents.tokens.len()
    );
    Ok(())
}

/// The one shape of file the resolver refuses outright: tokens with no key to
/// check them against would authenticate nobody, so it would serve with
/// authorization silently off. Better to hear it from the tool that wrote the
/// file than from a sidecar that will not start.
fn validate_token_table(contents: &Contents) -> Result<(), Failure> {
    if !contents.tokens.is_empty() && contents.token_key.is_none() {
        return Err(Failure::Usage(format!(
            "this file has {} token(s) but no token_key, so no token could ever authenticate. \
             The resolver refuses files in that shape. Add a token_key (`kallisto-ctl gen-key`), \
             or remove the tokens.",
            contents.tokens.len()
        )));
    }
    for (name, policies) in &contents.tokens {
        for policy in policies {
            if !contents.policies.contains_key(policy) {
                // Not fatal: an undefined policy grants nothing, so the failure
                // is already in the safe direction. Worth saying out loud,
                // because it looks exactly like a working token until it is
                // used.
                eprintln!(
                    "kallisto-ctl: warning: token {}… refers to policy {policy:?}, \
                     which this file does not define",
                    &name[..name.len().min(8)]
                );
            }
        }
    }
    Ok(())
}

pub fn run_verify(args: &Args) -> Result<(), Failure> {
    args.reject_unknown(&["--in"])?;
    let input = args.required("--in")?;
    let bytes = read(input)?;
    let key = seal_key()?;

    let opened = open(&bytes, &key, None).map_err(|e| describe(e, input))?;
    let view = opened
        .view()
        .map_err(|e| Failure::Failed(format!("{input} decrypted but is not readable: {e}")))?;

    // Counts and names of *policies*, never a secret path and never a value. A
    // policy name is written by the operator and describes a role, so it is the
    // one thing here that is safe and useful to print.
    println!("{input}: authentic");
    println!("  version   {}", opened.content_version());
    println!("  secrets   {}", view.secrets.len());
    println!("  policies  {}", view.policies.len());
    println!("  tokens    {}", view.tokens.len());
    println!(
        "  authorization {}",
        if view.token_key.is_some() {
            "enforced"
        } else {
            "none — every read is permitted"
        }
    );
    Ok(())
}

pub fn run_bump_version(args: &Args) -> Result<(), Failure> {
    args.reject_unknown(&["--in", "--to"])?;
    let input = args.required("--in")?;
    let bytes = read(input)?;
    let key = seal_key()?;

    let opened = open(&bytes, &key, None).map_err(|e| describe(e, input))?;
    let held = opened.content_version();
    let next = match args.number("--to")? {
        Some(to) if to <= held => {
            return Err(Failure::Usage(format!(
                "{input} holds version {held}; --to {to} would go backwards and the resolver \
                 would refuse the result"
            )));
        }
        Some(to) => to,
        None => held + 1,
    };

    // The version is in the header *and* in the body, and the header is the
    // AEAD's additional data — so there is no way to edit it in place. The file
    // has to be opened and sealed again, which is why this needs the key.
    let view = opened
        .view()
        .map_err(|e| Failure::Failed(format!("{input} decrypted but is not readable: {e}")))?;
    let mut contents = owned(&view);
    contents.version = next;

    let resealed = seal(&contents, &key)
        .map_err(|e| Failure::Failed(format!("cannot re-seal {input}: {e}")))?;
    write_atomically(Path::new(input), &resealed)?;

    println!("{input}: version {held} → {next}");
    Ok(())
}

/// Turns the borrowed view back into an owned `Contents` so it can be sealed
/// again.
///
/// This is the copy `Snapshot::build` goes out of its way *not* to make. It is
/// fine here and nowhere near the server: see the note at the top of this file.
fn owned(view: &core_crypto::View<'_>) -> Contents {
    Contents {
        version: view.version,
        secrets: view
            .secrets
            .iter()
            .map(|(path, value)| {
                (
                    (*path).to_string(),
                    serde_json::from_str(value.get()).unwrap_or(serde_json::Value::Null),
                )
            })
            .collect(),
        policies: view.policies.clone(),
        tokens: view.tokens.clone(),
        token_key: view.token_key.map(str::to_string),
    }
}

pub fn run_validate(args: &Args) -> Result<(), Failure> {
    args.reject_unknown(&["--config"])?;
    let path = args.required("--config")?;

    let file = config::read_file(Path::new(path)).map_err(|e| Failure::Failed(e.to_string()))?;
    // Resolved with no environment and no flags on purpose: this answers "is
    // the file itself sound", and folding in whatever happens to be set in this
    // shell would make the answer depend on where it was asked.
    let resolved = Config::resolve(
        file,
        config::Overrides::default(),
        config::Overrides::default(),
    )
    .map_err(|e| Failure::Failed(e.to_string()))?;

    println!("{path}: valid");
    println!("  listen    {}", resolved.listen);
    println!("  workers   {}", resolved.workers);
    println!("  mount     {}", resolved.mount);
    println!("  source    {:?}", resolved.source);
    println!("  refresh   every {}s", resolved.refresh_interval.as_secs());
    println!(
        "  cache     {}",
        resolved.cache_path.as_deref().map_or(
            "none — this machine cannot cold-start without the source".to_string(),
            |p| p.display().to_string()
        )
    );
    println!(
        "  limits    {}/s per worker, burst {}",
        resolved.limits.requests_per_second, resolved.limits.burst
    );
    println!(
        "  log       {}, queue {}",
        if resolved.log.enabled {
            "on"
        } else {
            "off — no reads will be recorded"
        },
        resolved.log.queue_capacity
    );
    Ok(())
}

pub fn run_open(args: &Args) -> Result<(), Failure> {
    args.reject_unknown(&["--in", "--yes-print-secrets-to-stdout"])?;
    let input = args.required("--in")?;

    // The flag is the whole safety mechanism, and it is spelled out rather than
    // shortened so that it cannot be typed by accident or left in a script
    // without somebody noticing it on review.
    if !args.has("--yes-print-secrets-to-stdout") {
        return Err(Failure::Usage(
            "open prints every secret in the file in the clear, which means into your \
             terminal scrollback and your shell's session log. If that is what you want, \
             pass --yes-print-secrets-to-stdout."
                .to_string(),
        ));
    }

    let bytes = read(input)?;
    let key = seal_key()?;
    let opened = open(&bytes, &key, None).map_err(|e| describe(e, input))?;
    let view = opened
        .view()
        .map_err(|e| Failure::Failed(format!("{input} decrypted but is not readable: {e}")))?;

    let rendered = serde_json::to_string_pretty(&owned(&view))
        .map_err(|_| Failure::Failed("cannot render the contents".to_string()))?;
    println!("{rendered}");
    Ok(())
}

pub fn run_mint_token(args: &Args) -> Result<(), Failure> {
    args.reject_unknown(&["--in", "--policy"])?;
    let policies = args.list("--policy");
    if policies.is_empty() {
        return Err(Failure::Usage(
            "--policy is required, at least once: a token with no policies grants nothing"
                .to_string(),
        ));
    }

    // The key comes from the file when there is one, because that is where it
    // lives, and from the environment otherwise — for minting the first token
    // before any file carries a table.
    let key = match args.value("--in") {
        Some(input) => {
            let bytes = read(input)?;
            let seal_key = seal_key()?;
            let opened = open(&bytes, &seal_key, None).map_err(|e| describe(e, input))?;
            let view = opened.view().map_err(|e| {
                Failure::Failed(format!("{input} decrypted but is not readable: {e}"))
            })?;
            let hex = view.token_key.ok_or_else(|| {
                Failure::Usage(format!(
                    "{input} carries no token_key, so it has no token table to add to. \
                     Run `kallisto-ctl gen-key`, put the result in the plaintext file's \
                     token_key field, and seal it."
                ))
            })?;
            TokenKey::from_hex(hex)
                .map_err(|e| Failure::Failed(format!("{input}'s token_key is unusable: {e}")))?
        }
        None => {
            let hex = std::env::var(TOKEN_KEY_ENV).map_err(|_| {
                Failure::Usage(format!(
                    "pass --in <file.kal> to take the token key from the file, or set \
                     {TOKEN_KEY_ENV} to mint against a key that is not sealed yet"
                ))
            })?;
            TokenKey::from_hex(&hex)
                .map_err(|e| Failure::Usage(format!("{TOKEN_KEY_ENV} is not usable: {e}")))?
        }
    };

    let token = random_token()?;
    let hash = key.hash_hex(&token);

    // The token to stdout so it can be piped into a secret store; everything
    // else to stderr so that piping gets the token and nothing else.
    eprintln!(
        "kallisto-ctl: this token is shown once. Kallisto stores only its keyed hash, so \
         there is no way to recover it — losing it means minting another."
    );
    eprintln!();
    eprintln!("Add to the plaintext file's \"tokens\" map, then re-seal:");
    eprintln!();
    eprintln!(
        "    {:?}: {}",
        hash,
        serde_json::to_string(&policies).unwrap_or_default()
    );
    eprintln!();
    eprintln!("The token itself, for the application (on stdout):");
    println!("{token}");
    Ok(())
}

/// 256 bits from the system RNG, prefixed the way Vault prefixes its own so
/// that a token is recognisable in a configuration file for what it is.
fn random_token() -> Result<String, Failure> {
    let mut bytes = [0u8; 32];
    aws_lc_rs::rand::fill(&mut bytes)
        .map_err(|_| Failure::Failed("the system random number generator is unavailable".into()))?;
    Ok(format!("s.{}", core_crypto::hex::encode(&bytes)))
}

pub fn run_gen_key(args: &Args) -> Result<(), Failure> {
    args.reject_unknown(&[])?;
    let mut bytes = [0u8; 32];
    aws_lc_rs::rand::fill(&mut bytes)
        .map_err(|_| Failure::Failed("the system random number generator is unavailable".into()))?;
    let hex = core_crypto::hex::encode(&bytes);
    zeroize::Zeroize::zeroize(&mut bytes);

    eprintln!(
        "kallisto-ctl: 32 bytes from the system RNG, hex-encoded. Usable as {SEAL_KEY_ENV} or \
         as a file's token_key. It is printed on stdout and nowhere else — this tool does not \
         store it."
    );
    println!("{hex}");
    Ok(())
}

/// Turns a `SealError` into something an operator can act on.
///
/// Each of these means a different next move, which is the reason `SealError`
/// distinguishes them at all, and none of them carries file content.
fn describe(error: SealError, path: &str) -> Failure {
    let advice = match &error {
        SealError::BadMagic => " — this is not a Kallisto sealed file",
        SealError::AuthFailed => {
            " — either the key is wrong or the file has been altered. Both look identical \
             from here, and that is the point of the tag."
        }
        SealError::Truncated { .. } => " — the file is incomplete; an upload may have been cut off",
        _ => "",
    };
    Failure::Failed(format!("{path}: {error}{advice}"))
}
