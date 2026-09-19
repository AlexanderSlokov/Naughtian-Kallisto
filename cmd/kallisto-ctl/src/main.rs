//! `kallisto-ctl` — the offline half of Kallisto.
//!
//! The resolver only ever reads. Everything that *writes* a sealed file happens
//! here, on an operator's machine or in a CI job, and never over the network:
//! ADR-0015 D1 makes "cannot write" a property of the server, and this is where
//! the writing went instead.
//!
//! Two rules hold throughout, and both are enforced rather than documented:
//!
//! * **No key is ever a command-line argument.** Arguments are readable by
//!   every process on the host through `ps`, and they end up in shell history
//!   and CI logs. `--seal-key` and `--token-key` are refused by name so that
//!   reaching for them produces an explanation rather than a working command.
//! * **Nothing prints a secret unless asked in so many words.** `open` exists,
//!   because an operator sometimes has to see what is in the file, and it
//!   requires `--yes-print-secrets-to-stdout` to say so.
//!
//! What this is not: a TUI. ADR-0004 is still `suspended`, and the `ratatui`
//! stub that used to live here is gone.

mod args;
mod commands;

use std::process::ExitCode;

use args::Args;
use commands::Failure;

const USAGE: &str = "\
kallisto-ctl — offline tool for Kallisto's sealed secret file

USAGE
    kallisto-ctl <command> [options]

COMMANDS
    seal          --in <plain.json> --out <file.kal> [--version N] [--force]
                  Encrypt a plaintext secrets file. Refuses to write a version
                  the resolver would reject as a rollback, unless --force.

    verify        --in <file.kal>
                  Check the file's authentication tag and report what it holds.
                  Never prints a secret or a secret's path.

    bump-version  --in <file.kal> [--to N]
                  Raise the content version in place (default: +1). Needs the
                  key, because the version is authenticated.

    validate      --config <kallisto.yaml>
                  Parse and resolve a configuration, and print what the server
                  would actually do with it.

    mint-token    [--in <file.kal>] --policy <name> [--policy <name>...]
                  Generate a token and print the line to paste into the
                  plaintext file's token table. The token is shown once.

    gen-key       Print 32 random bytes, hex-encoded, for use as a seal key or
                  a file's token_key.

    open          --in <file.kal> --yes-print-secrets-to-stdout
                  Print the decrypted contents. Exactly as dangerous as it
                  sounds, which is why the flag is spelled out.

ENVIRONMENT
    KALLISTO_SEAL_KEY   the 32-byte seal key, hex-encoded. Required by every
                        command that opens or writes a sealed file.
    KALLISTO_TOKEN_KEY  a token key for `mint-token` when no sealed file
                        carries one yet.

    Keys are read from the environment only. There is no flag for them, and
    passing one is an error: command-line arguments are world-readable.
";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.is_empty()
        || arguments
            .iter()
            .any(|a| a == "-h" || a == "--help" || a == "help")
    {
        println!("{USAGE}");
        return if arguments.is_empty() {
            ExitCode::from(2)
        } else {
            ExitCode::SUCCESS
        };
    }

    let parsed = match Args::parse(arguments.into_iter()) {
        Ok(parsed) => parsed,
        Err(e) => return fail(Failure::Usage(e.0)),
    };

    let result = match parsed.command.as_str() {
        "seal" => commands::run_seal(&parsed),
        "verify" => commands::run_verify(&parsed),
        "bump-version" => commands::run_bump_version(&parsed),
        "validate" => commands::run_validate(&parsed),
        "mint-token" => commands::run_mint_token(&parsed),
        "gen-key" => commands::run_gen_key(&parsed),
        "open" => commands::run_open(&parsed),
        other => Err(Failure::Usage(format!(
            "unknown command {other:?}\n\n{USAGE}"
        ))),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => fail(failure),
    }
}

/// Usage errors exit 2 and real failures exit 1, so a Makefile or CI job can
/// tell "you called it wrong" from "it did not work".
fn fail(failure: Failure) -> ExitCode {
    match failure {
        Failure::Usage(message) => {
            eprintln!("kallisto-ctl: {message}");
            ExitCode::from(2)
        }
        Failure::Failed(message) => {
            eprintln!("kallisto-ctl: {message}");
            ExitCode::FAILURE
        }
    }
}
