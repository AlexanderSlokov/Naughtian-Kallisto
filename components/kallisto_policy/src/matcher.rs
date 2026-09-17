//! Vault's policy path syntax, including the KV-v2 quirk (ADR-0015 D8).
//!
//! The quirk is the point. In KV version 2 the capability to read a secret is
//! written against `secret/data/payment/db`, and the capability to list it
//! against `secret/metadata/payment/db`, because the API URL has a `data/` or
//! `metadata/` segment the user never typed. Everyone who has written a Vault
//! policy has forgotten this at least once. Kallisto copies it exactly, so a
//! policy written for a real Vault works here unchanged, and so a policy
//! written here keeps working when somebody outgrows Kallisto and moves to
//! OpenBao.
//!
//! Three capabilities, because three is all a read-only resolver can mean:
//! `read`, `list`, `deny`. A Vault policy carrying `create`, `update` or
//! `sudo` still parses — those capabilities simply grant nothing, which is the
//! truth: there is no write path for them to authorise.
//!
//! # Where this deliberately differs from Vault
//!
//! Real Vault resolves a conflict by **path specificity**: the most specific
//! matching rule wins, so a narrow `read` grant beats a broad `deny`. Kallisto
//! gives `deny` precedence over every other rule, however broad. The two agree
//! on every policy that does not contain a contradiction; where they disagree,
//! Kallisto refuses a request Vault would have allowed. That direction is
//! deliberate — a 403 is a visible, diagnosable failure, and the alternative is
//! serving a secret because a rule twelve lines further up was worded more
//! precisely.

use core_crypto::PolicyRule;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// `GET /v1/{mount}/data/{path}` and `GET /v1/{mount}/metadata/{path}`.
    Read,
    /// `LIST /v1/{mount}/metadata/{path}` and its `?list=true` spelling.
    List,
}

/// One rule with its path pattern already parsed.
///
/// Compiled when a snapshot is built, not when a request arrives: the pattern
/// text is the same for the life of a file, and splitting a string on every
/// read to answer a question whose answer cannot have changed is the kind of
/// work ADR-0016 QĐ-3 exists to keep off this path.
#[derive(Debug, Clone)]
pub struct CompiledRule {
    pattern: Pattern,
    read: bool,
    list: bool,
    deny: bool,
}

impl CompiledRule {
    pub fn compile(rule: &PolicyRule) -> Self {
        let mut compiled = Self {
            pattern: Pattern::compile(&rule.path),
            read: false,
            list: false,
            deny: false,
        };
        for capability in &rule.capabilities {
            match capability.as_str() {
                "read" => compiled.read = true,
                "list" => compiled.list = true,
                "deny" => compiled.deny = true,
                // `create`, `update`, `delete`, `patch`, `sudo`: recognised by
                // Vault, meaningless here, and not an error. A policy file is
                // meant to be portable in both directions.
                _ => {}
            }
        }
        compiled
    }

    fn grants(&self, capability: Capability) -> bool {
        match capability {
            Capability::Read => self.read,
            Capability::List => self.list,
        }
    }
}

/// Every rule a single token carries, flattened across all of its policies.
///
/// Flattened at snapshot-build time so that a request costs one walk of one
/// list, rather than a walk of policy names followed by a map lookup each.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    rules: Vec<CompiledRule>,
}

impl RuleSet {
    pub fn from_rules<'a>(rules: impl IntoIterator<Item = &'a PolicyRule>) -> Self {
        Self {
            rules: rules.into_iter().map(CompiledRule::compile).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// `path` is the full Vault-style path — `secret/data/payment/db`, with the
    /// mount and the `data/` or `metadata/` segment already in it.
    ///
    /// Default deny: a path no rule mentions is refused.
    pub fn allows(&self, path: &str, capability: Capability) -> bool {
        let mut granted = false;
        for rule in &self.rules {
            if !rule.pattern.matches(path) {
                continue;
            }
            if rule.deny {
                return false;
            }
            granted |= rule.grants(capability);
        }
        granted
    }
}

// -----------------------------------------------------------------------------
// Path patterns
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    /// `+` — exactly one path segment, any content.
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    /// The segments before any trailing `*`.
    segments: Vec<Segment>,
    /// `Some(prefix)` when the pattern ended in `*`. Whatever is left of the
    /// candidate after `segments` must start with `prefix`, and `*` matches
    /// across `/` exactly as it does in Vault: `secret/data/app*` matches
    /// `secret/data/app/db`.
    glob: Option<String>,
}

impl Pattern {
    fn compile(path: &str) -> Self {
        let (body, globbed) = match path.strip_suffix('*') {
            Some(rest) => (rest, true),
            None => (path, false),
        };

        let mut segments: Vec<Segment> = body
            .split('/')
            .map(|s| {
                if s == "+" {
                    Segment::Any
                } else {
                    Segment::Literal(s.to_string())
                }
            })
            .collect();

        // With a trailing `*`, the final segment is not a segment at all: it is
        // the partial text the remainder has to start with. For the common
        // `secret/data/app/*` that text is empty.
        let glob = if globbed {
            Some(match segments.pop() {
                Some(Segment::Literal(text)) => text,
                // `secret/+*` — nonsense, but not a reason to panic.
                Some(Segment::Any) | None => String::new(),
            })
        } else {
            None
        };

        Self { segments, glob }
    }

    fn matches(&self, candidate: &str) -> bool {
        let parts: Vec<&str> = candidate.split('/').collect();
        if parts.len() < self.segments.len() {
            return false;
        }

        let mut consumed = 0usize;
        for (segment, part) in self.segments.iter().zip(&parts) {
            if let Segment::Literal(text) = segment
                && text != part
            {
                return false;
            }
            consumed += part.len() + 1; // the segment and its trailing '/'
        }

        match &self.glob {
            // No glob: the candidate must end exactly where the pattern does.
            None => parts.len() == self.segments.len(),
            Some(prefix) => {
                // Nothing left for the glob to stand for. `secret/data/app/*`
                // does not match `secret/data/app`, which is Vault's behaviour
                // and the one people rely on to keep a parent path private.
                if parts.len() == self.segments.len() {
                    return false;
                }
                candidate[consumed..].starts_with(prefix)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(path: &str, capabilities: &[&str]) -> PolicyRule {
        PolicyRule {
            path: path.to_string(),
            capabilities: capabilities.iter().map(|c| (*c).to_string()).collect(),
        }
    }

    fn set(rules: &[PolicyRule]) -> RuleSet {
        RuleSet::from_rules(rules)
    }

    #[test]
    fn an_exact_path_matches_only_itself() {
        let rules = set(&[rule("secret/data/app/db", &["read"])]);
        assert!(rules.allows("secret/data/app/db", Capability::Read));
        assert!(!rules.allows("secret/data/app/dbx", Capability::Read));
        assert!(!rules.allows("secret/data/app/db/more", Capability::Read));
        assert!(!rules.allows("secret/data/app", Capability::Read));
    }

    #[test]
    fn a_trailing_glob_matches_across_slashes_as_in_vault() {
        let rules = set(&[rule("secret/data/app/*", &["read"])]);
        assert!(rules.allows("secret/data/app/db", Capability::Read));
        assert!(rules.allows("secret/data/app/sub/deep", Capability::Read));
        // The parent itself is not covered by its own children's glob.
        assert!(!rules.allows("secret/data/app", Capability::Read));
        assert!(!rules.allows("secret/data/other/db", Capability::Read));
    }

    #[test]
    fn a_glob_can_start_mid_segment() {
        let rules = set(&[rule("secret/data/app*", &["read"])]);
        assert!(rules.allows("secret/data/application", Capability::Read));
        assert!(rules.allows("secret/data/app/db", Capability::Read));
        assert!(!rules.allows("secret/data/other", Capability::Read));
    }

    #[test]
    fn plus_matches_exactly_one_segment() {
        let rules = set(&[rule("secret/data/+/db", &["read"])]);
        assert!(rules.allows("secret/data/app/db", Capability::Read));
        assert!(rules.allows("secret/data/other/db", Capability::Read));
        // One segment, not two.
        assert!(!rules.allows("secret/data/app/sub/db", Capability::Read));
        assert!(!rules.allows("secret/data/db", Capability::Read));
    }

    #[test]
    fn plus_and_a_glob_compose() {
        let rules = set(&[rule("secret/data/+/shared/*", &["read"])]);
        assert!(rules.allows("secret/data/app/shared/db", Capability::Read));
        assert!(rules.allows("secret/data/web/shared/a/b", Capability::Read));
        assert!(!rules.allows("secret/data/app/private/db", Capability::Read));
    }

    /// ADR-0015 D8: this is the quirk people forget, and copying it is the
    /// whole compatibility argument.
    #[test]
    fn reading_and_listing_are_granted_on_different_paths() {
        let rules = set(&[
            rule("secret/data/payment/*", &["read"]),
            rule("secret/metadata/payment/*", &["list"]),
        ]);
        assert!(rules.allows("secret/data/payment/db", Capability::Read));
        assert!(rules.allows("secret/metadata/payment/db", Capability::List));

        // A read grant on `data/` does not let you enumerate, and a list grant
        // on `metadata/` does not let you read.
        assert!(!rules.allows("secret/metadata/payment/db", Capability::Read));
        assert!(!rules.allows("secret/data/payment/db", Capability::List));
    }

    /// ADR-0013 E3, in the unit that implements it.
    #[test]
    fn deny_beats_a_grant_however_broad_the_deny_is() {
        let rules = set(&[
            rule("secret/data/app/*", &["read"]),
            rule("secret/data/app/private", &["deny"]),
        ]);
        assert!(rules.allows("secret/data/app/public", Capability::Read));
        assert!(!rules.allows("secret/data/app/private", Capability::Read));

        // And in the other order, where Vault's specificity rule would have
        // allowed it. Documented at the top of this file.
        let inverted = set(&[
            rule("secret/data/app/*", &["deny"]),
            rule("secret/data/app/public", &["read"]),
        ]);
        assert!(!inverted.allows("secret/data/app/public", Capability::Read));
    }

    /// Rule order must not change the answer, or a policy file becomes a
    /// program.
    #[test]
    fn the_order_rules_are_written_in_does_not_matter() {
        let forward = set(&[
            rule("secret/data/app/*", &["read"]),
            rule("secret/data/app/private", &["deny"]),
        ]);
        let backward = set(&[
            rule("secret/data/app/private", &["deny"]),
            rule("secret/data/app/*", &["read"]),
        ]);
        for path in ["secret/data/app/public", "secret/data/app/private"] {
            assert_eq!(
                forward.allows(path, Capability::Read),
                backward.allows(path, Capability::Read),
                "order changed the answer for {path}"
            );
        }
    }

    #[test]
    fn a_path_no_rule_mentions_is_refused() {
        let rules = set(&[rule("secret/data/app/*", &["read"])]);
        assert!(!rules.allows("secret/data/other/db", Capability::Read));
        assert!(!RuleSet::default().allows("secret/data/app/db", Capability::Read));
    }

    /// A policy lifted from a real Vault carries capabilities this cannot
    /// honour. It must parse, and it must not grant anything by accident.
    #[test]
    fn vaults_write_capabilities_parse_and_grant_nothing() {
        let rules = set(&[rule(
            "secret/data/app/*",
            &["create", "update", "delete", "patch", "sudo"],
        )]);
        assert!(!rules.allows("secret/data/app/db", Capability::Read));
        assert!(!rules.allows("secret/data/app/db", Capability::List));
        assert_eq!(rules.len(), 1, "the rule must still be present");
    }

    #[test]
    fn a_bare_glob_covers_everything() {
        let rules = set(&[rule("*", &["read"])]);
        assert!(rules.allows("secret/data/app/db", Capability::Read));
        assert!(rules.allows("anything", Capability::Read));
    }
}
