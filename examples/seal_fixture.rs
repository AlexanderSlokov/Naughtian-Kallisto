//! Seals a plaintext JSON file, so that the resolver can be exercised before
//! `kallisto-ctl` exists (M7 of the duck plan).
//!
//! An example rather than a binary on purpose: it is not shipped, and the real
//! command will have to answer questions this does not — refusing to print
//! secrets, validating a configuration, bumping a version in place.
//!
//!     KALLISTO_SEAL_KEY=$(openssl rand -hex 32) \
//!         cargo run --example seal_fixture -- plain.json secrets.kal

use std::process::ExitCode;

use core_crypto::{Contents, SealKey, seal};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output] = args.as_slice() else {
        eprintln!("usage: seal_fixture <plain.json> <sealed.kal>");
        return ExitCode::from(2);
    };

    let Ok(hex) = std::env::var("KALLISTO_SEAL_KEY") else {
        eprintln!("KALLISTO_SEAL_KEY is not set");
        return ExitCode::from(2);
    };
    let key = match SealKey::from_hex(&hex) {
        Ok(key) => key,
        Err(e) => {
            eprintln!("KALLISTO_SEAL_KEY is not usable: {e}");
            return ExitCode::from(2);
        }
    };

    let text = match std::fs::read_to_string(input) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("cannot read {input}: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Deliberately terse: a parse error here quotes the offending value, and
    // the offending value is a secret.
    let Ok(contents) = serde_json::from_str::<Contents>(&text) else {
        eprintln!("{input} is not a valid plaintext secrets file");
        return ExitCode::FAILURE;
    };

    match seal(&contents, &key).and_then(|bytes| {
        std::fs::write(output, bytes).map_err(|_| core_crypto::SealError::RandomUnavailable)
    }) {
        Ok(()) => {
            println!("sealed version {} into {output}", contents.version);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("could not seal: {e}");
            ExitCode::FAILURE
        }
    }
}
