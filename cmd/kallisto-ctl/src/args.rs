//! A flag parser the size of the problem.
//!
//! Same spelling rules as the server's (`--flag value` and `--flag=value` both
//! work), because an operator who has learned one should not have to learn the
//! other. Deliberately not a derive-macro argument crate: this is seven
//! subcommands with two flags each, and the dependency would be larger than the
//! program.

use std::collections::BTreeMap;

/// `Debug` is safe here only because key-bearing flags are refused during
/// parsing: nothing that reaches these fields is secret material.
#[derive(Debug)]
pub struct Args {
    pub command: String,
    values: BTreeMap<String, String>,
    flags: Vec<String>,
}

#[derive(Debug)]
pub struct ArgError(pub String);

impl std::fmt::Display for ArgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Flags that take a value. Anything else beginning with `--` is a bare flag,
/// so a typo in a value-taking flag is caught here rather than silently
/// swallowing the next argument.
const TAKES_VALUE: &[&str] = &["--in", "--out", "--config", "--version", "--to", "--policy"];

impl Args {
    pub fn parse<I: Iterator<Item = String>>(args: I) -> Result<Self, ArgError> {
        let mut args = args.peekable();
        let command = args
            .next()
            .ok_or_else(|| ArgError("no command given".to_string()))?;

        let mut values = BTreeMap::new();
        let mut flags = Vec::new();
        let mut repeated: BTreeMap<String, Vec<String>> = BTreeMap::new();

        while let Some(arg) = args.next() {
            let (flag, inline) = match arg.split_once('=') {
                Some((f, v)) => (f.to_string(), Some(v.to_string())),
                None => (arg, None),
            };

            // The one flag with a message of its own. Someone reaching for it
            // is one step from putting a key in a shell history file, a CI log
            // and a process listing at once.
            if flag == "--seal-key" || flag == "--token-key" {
                return Err(ArgError(format!(
                    "{flag} is never a command-line argument — every process on this host can \
                     read those. Set the environment variable instead."
                )));
            }

            if TAKES_VALUE.contains(&flag.as_str()) {
                let value = match inline {
                    Some(v) => v,
                    None => args
                        .next()
                        .ok_or_else(|| ArgError(format!("{flag} requires a value")))?,
                };
                repeated
                    .entry(flag.clone())
                    .or_default()
                    .push(value.clone());
                values.insert(flag, value);
            } else if flag.starts_with("--") {
                if inline.is_some() {
                    return Err(ArgError(format!("{flag} does not take a value")));
                }
                flags.push(flag);
            } else {
                return Err(ArgError(format!("unexpected argument {flag:?}")));
            }
        }

        // `--policy` is the only flag that may be given more than once.
        if let Some(policies) = repeated.get("--policy") {
            values.insert("--policy".to_string(), policies.join("\u{1}"));
        }

        Ok(Self {
            command,
            values,
            flags,
        })
    }

    pub fn value(&self, flag: &str) -> Option<&str> {
        self.values.get(flag).map(String::as_str)
    }

    pub fn required(&self, flag: &str) -> Result<&str, ArgError> {
        self.value(flag)
            .ok_or_else(|| ArgError(format!("{flag} is required")))
    }

    pub fn number(&self, flag: &str) -> Result<Option<u64>, ArgError> {
        match self.value(flag) {
            None => Ok(None),
            Some(raw) => raw
                .parse()
                .map(Some)
                .map_err(|_| ArgError(format!("{flag} expects a number, got {raw:?}"))),
        }
    }

    pub fn list(&self, flag: &str) -> Vec<&str> {
        self.value(flag)
            .map(|joined| joined.split('\u{1}').collect())
            .unwrap_or_default()
    }

    pub fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }

    /// Anything left over is a typo, and a typo in a destructive command should
    /// stop it rather than be ignored.
    pub fn reject_unknown(&self, known: &[&str]) -> Result<(), ArgError> {
        for flag in self.flags.iter().chain(self.values.keys()) {
            if !known.contains(&flag.as_str()) {
                return Err(ArgError(format!("unrecognised argument {flag:?}")));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, ArgError> {
        Args::parse(args.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn both_spellings_of_a_flag_work() {
        let a = parse(&["seal", "--in", "a.json", "--out=b.kal"]).unwrap();
        assert_eq!(a.command, "seal");
        assert_eq!(a.value("--in"), Some("a.json"));
        assert_eq!(a.value("--out"), Some("b.kal"));
    }

    #[test]
    fn a_bare_flag_is_not_confused_with_one_that_takes_a_value() {
        let a = parse(&["open", "--in", "x.kal", "--yes-print-secrets-to-stdout"]).unwrap();
        assert!(a.has("--yes-print-secrets-to-stdout"));
        assert_eq!(a.value("--in"), Some("x.kal"));
    }

    #[test]
    fn a_value_flag_with_nothing_after_it_is_an_error() {
        parse(&["seal", "--in"]).unwrap_err();
    }

    #[test]
    fn policies_accumulate() {
        let a = parse(&["mint-token", "--policy", "app", "--policy=db"]).unwrap();
        assert_eq!(a.list("--policy"), vec!["app", "db"]);
    }

    /// The flag that must never exist. A key on the command line is readable by
    /// every process on the host through `ps`.
    #[test]
    fn a_key_on_the_command_line_is_refused_by_name() {
        for spelling in ["--seal-key", "--token-key"] {
            let err = parse(&["seal", spelling, "0011"]).unwrap_err();
            assert!(err.0.contains("never a command-line argument"), "{err}");
            assert!(!err.0.contains("0011"), "the error echoed the key: {err}");
        }
    }

    #[test]
    fn a_typo_is_refused_rather_than_ignored() {
        let a = parse(&["verify", "--in", "x.kal", "--forse"]).unwrap();
        a.reject_unknown(&["--in", "--force"]).unwrap_err();
        a.reject_unknown(&["--in", "--forse"]).unwrap();
    }

    #[test]
    fn a_number_flag_reports_what_it_could_not_read() {
        let a = parse(&["seal", "--version", "abc"]).unwrap();
        a.number("--version").unwrap_err();
        let b = parse(&["seal", "--version", "9"]).unwrap();
        assert_eq!(b.number("--version").unwrap(), Some(9));
    }
}
