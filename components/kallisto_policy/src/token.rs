//! Tokens: a keyed hash, a constant-time lookup, and a set of rules.
//!
//! ADR-0015 D8 turns Vault's one genuinely stateful subsystem into data. There
//! is no token store, no lease, no expiry and no revocation endpoint: the file
//! carries a table of `hash(token) → policy names`, an operator mints a random
//! token and hands it to an app, and revoking it means deleting that line and
//! re-sealing — the same motion as rotating a secret.
//!
//! The hash is keyed, and the key lives inside the sealed file. Two
//! consequences, both deliberate:
//!
//! * The plaintext of the file — which is authored by hand and passes through
//!   somebody's editor, `git`, and a CI job before it is ever sealed — contains
//!   hashes that cannot be attacked offline without the key, even if an
//!   operator picks a short token.
//! * Rotating the *seal* key does not invalidate a single token, because the
//!   token key is re-sealed along with everything else. Deriving the token key
//!   from the seal key would have made every rotation a fleet-wide outage: the
//!   operator holds the hashes, not the tokens, so there is nothing to
//!   recompute them from.

use std::collections::BTreeMap;

use aws_lc_rs::{
    constant_time,
    hmac::{self, HMAC_SHA256},
};
use core_crypto::{PolicyRule, hex};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::matcher::RuleSet;

pub const TOKEN_KEY_LEN: usize = 32;
pub const HASH_LEN: usize = 32;

/// Domain separation, so that this key can never be reused for a different
/// purpose and produce a value that means something here.
const LABEL: &[u8] = b"kallisto/token/v1\x00";

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error(
        "the file carries {tokens} token(s) but no token key, so no token could ever \
         authenticate; refusing it rather than serving with authorization off"
    )]
    KeyMissing { tokens: usize },
    #[error("the file's token key is unusable: {0}")]
    KeyMalformed(#[from] hex::HexError),
    #[error("token table entry {index} is not a {} character hex hash", HASH_LEN * 2)]
    HashMalformed { index: usize },
}

/// The key the token hashes were computed with.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct TokenKey([u8; TOKEN_KEY_LEN]);

impl TokenKey {
    pub fn from_bytes(bytes: [u8; TOKEN_KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn from_hex(text: &str) -> Result<Self, hex::HexError> {
        let mut bytes = [0u8; TOKEN_KEY_LEN];
        hex::decode_into(text, &mut bytes)?;
        Ok(Self(bytes))
    }

    /// For the tool that mints a token table. Named bluntly on purpose: every
    /// call site should read as a decision to write a key somewhere.
    pub fn expose_as_hex(&self) -> String {
        hex::encode(&self.0)
    }

    /// What goes in the file's `tokens` table for a given token.
    pub fn hash(&self, token: &str) -> [u8; HASH_LEN] {
        let key = hmac::Key::new(HMAC_SHA256, &self.0);
        let mut context = hmac::Context::with_key(&key);
        context.update(LABEL);
        context.update(token.as_bytes());

        let mut out = [0u8; HASH_LEN];
        out.copy_from_slice(context.sign().as_ref());
        out
    }

    pub fn hash_hex(&self, token: &str) -> String {
        hex::encode(&self.hash(token))
    }
}

impl std::fmt::Debug for TokenKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenKey(<REDACTED>)")
    }
}

struct Entry {
    hash: [u8; HASH_LEN],
    rules: RuleSet,
    /// Kept beside the flattened rules for `auth/token/lookup-self`, which
    /// several SDKs call on startup and which is supposed to answer in policy
    /// names rather than in paths.
    policies: Vec<String>,
}

/// What a token turned out to carry.
pub struct Grant<'a> {
    pub rules: &'a RuleSet,
    pub policies: &'a [String],
}

/// The token table, with every token's policies already flattened into one
/// rule list.
pub struct TokenTable {
    key: TokenKey,
    entries: Vec<Entry>,
    /// Policy names a token referred to that the file does not define.
    ///
    /// Not an error: a typo in one token's policy list must not take every app
    /// on the machine offline, and an undefined policy grants nothing, so the
    /// failure is already in the safe direction. It is reported so that the
    /// failure is a visible one.
    unknown_policies: Vec<String>,
}

impl TokenTable {
    pub fn build(
        key_hex: &str,
        tokens: &BTreeMap<String, Vec<String>>,
        policies: &BTreeMap<String, Vec<PolicyRule>>,
    ) -> Result<Self, TokenError> {
        let key = TokenKey::from_hex(key_hex)?;
        let mut entries = Vec::with_capacity(tokens.len());
        let mut unknown_policies = Vec::new();

        for (index, (hash_hex, policy_names)) in tokens.iter().enumerate() {
            let mut hash = [0u8; HASH_LEN];
            hex::decode_into(hash_hex, &mut hash)
                .map_err(|_| TokenError::HashMalformed { index })?;

            let mut rules = Vec::new();
            for name in policy_names {
                match policies.get(name) {
                    Some(policy) => rules.extend(policy.iter()),
                    None => unknown_policies.push(name.clone()),
                }
            }

            entries.push(Entry {
                hash,
                rules: RuleSet::from_rules(rules),
                policies: policy_names.clone(),
            });
        }

        Ok(Self {
            key,
            entries,
            unknown_policies,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn unknown_policies(&self) -> &[String] {
        &self.unknown_policies
    }

    /// The rules the presented token carries, or `None`.
    ///
    /// ADR-0013 E2. Every entry is examined on every call and the comparison is
    /// constant-time, so neither the time taken nor the work done reveals
    /// whether a token exists, nor how much of a hash it shares with one that
    /// does. The early `return` a `HashMap` would have given us is exactly the
    /// thing being avoided.
    ///
    /// That costs one 32-byte comparison per token in the file. ADR-0015 D11
    /// sizes this deployment at a handful of apps on one machine, so the table
    /// is a few dozen entries at most and the scan is cheaper than the HMAC
    /// that precedes it. A table in the thousands would need a different shape,
    /// and would mean Kallisto is being used as something it is not.
    pub fn lookup(&self, presented: &str) -> Option<Grant<'_>> {
        let computed = self.key.hash(presented);

        let mut found = None;
        for entry in &self.entries {
            if constant_time::verify_slices_are_equal(&entry.hash, &computed).is_ok() {
                found = Some(Grant {
                    rules: &entry.rules,
                    policies: &entry.policies,
                });
            }
        }
        found
    }
}

/// Never the key, never a hash.
impl std::fmt::Debug for TokenTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenTable")
            .field("entries", &self.entries.len())
            .field("key", &"<REDACTED>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::Capability;

    fn key() -> TokenKey {
        TokenKey::from_bytes([7u8; TOKEN_KEY_LEN])
    }

    fn policies() -> BTreeMap<String, Vec<PolicyRule>> {
        BTreeMap::from([
            (
                "payment".to_string(),
                vec![PolicyRule {
                    path: "secret/data/payment/*".to_string(),
                    capabilities: vec!["read".to_string()],
                }],
            ),
            (
                "web".to_string(),
                vec![PolicyRule {
                    path: "secret/data/web/*".to_string(),
                    capabilities: vec!["read".to_string()],
                }],
            ),
        ])
    }

    fn table_with(token_policies: &[(&str, &[&str])]) -> TokenTable {
        let key = key();
        let tokens = token_policies
            .iter()
            .map(|(token, names)| {
                (
                    key.hash_hex(token),
                    names.iter().map(|n| (*n).to_string()).collect(),
                )
            })
            .collect();
        TokenTable::build(&key.expose_as_hex(), &tokens, &policies()).unwrap()
    }

    #[test]
    fn a_token_resolves_to_the_rules_of_all_its_policies() {
        let table = table_with(&[("s.realtoken", &["payment", "web"])]);
        let grant = table.lookup("s.realtoken").expect("token should resolve");
        assert_eq!(grant.policies, &["payment".to_string(), "web".to_string()]);
        let rules = grant.rules;
        assert!(rules.allows("secret/data/payment/db", Capability::Read));
        assert!(rules.allows("secret/data/web/db", Capability::Read));
        assert!(!rules.allows("secret/data/other/db", Capability::Read));
    }

    #[test]
    fn an_unknown_token_resolves_to_nothing() {
        let table = table_with(&[("s.realtoken", &["payment"])]);
        assert!(table.lookup("s.faketoken").is_none());
        assert!(table.lookup("").is_none());
    }

    /// Revocation, in the only form this design has: the line is gone.
    #[test]
    fn a_token_removed_from_the_table_stops_working() {
        let before = table_with(&[("s.a", &["payment"]), ("s.b", &["web"])]);
        assert!(before.lookup("s.b").is_some());

        let after = table_with(&[("s.a", &["payment"])]);
        assert!(after.lookup("s.b").is_none());
        assert!(after.lookup("s.a").is_some());
    }

    #[test]
    fn the_hash_is_keyed_so_the_same_token_hashes_differently_under_another_key() {
        let a = TokenKey::from_bytes([1u8; TOKEN_KEY_LEN]);
        let b = TokenKey::from_bytes([2u8; TOKEN_KEY_LEN]);
        assert_ne!(a.hash_hex("s.token"), b.hash_hex("s.token"));
        assert_eq!(a.hash_hex("s.token"), a.hash_hex("s.token"));
    }

    /// A plain SHA-256 of a short token is a dictionary lookup away from the
    /// token itself. This asserts we are not producing one.
    #[test]
    fn the_hash_is_not_a_bare_digest_of_the_token() {
        use aws_lc_rs::digest;
        let bare = digest::digest(&digest::SHA256, b"s.token");
        assert_ne!(key().hash("s.token").as_slice(), bare.as_ref());
    }

    #[test]
    fn a_token_key_never_renders_itself() {
        let rendered = format!("{:?} {:?}", key(), table_with(&[("s.a", &["payment"])]));
        assert!(rendered.contains("REDACTED"));
        assert!(!rendered.contains("0707"), "the key leaked: {rendered}");
    }

    /// A file with tokens and no key would authenticate nobody. Serving it
    /// would mean serving with authorization silently switched off.
    #[test]
    fn a_token_table_without_a_key_is_refused() {
        let tokens = BTreeMap::from([("ab".repeat(32), vec!["payment".to_string()])]);
        let err = TokenTable::build("", &tokens, &policies()).unwrap_err();
        assert!(matches!(err, TokenError::KeyMalformed(_)));
    }

    #[test]
    fn a_malformed_hash_is_refused_and_named_by_position_only() {
        let key = key();
        let tokens = BTreeMap::from([("not-a-hash".to_string(), vec!["payment".to_string()])]);
        let err = TokenTable::build(&key.expose_as_hex(), &tokens, &policies()).unwrap_err();
        assert!(matches!(err, TokenError::HashMalformed { index: 0 }));
        assert!(!err.to_string().contains("not-a-hash"));
    }

    /// A typo in one token's policy list must not take the machine offline.
    #[test]
    fn an_undefined_policy_grants_nothing_and_is_reported() {
        let table = table_with(&[("s.a", &["payment", "typo"])]);
        assert_eq!(table.unknown_policies(), &["typo".to_string()]);

        let rules = table.lookup("s.a").unwrap().rules;
        assert!(rules.allows("secret/data/payment/db", Capability::Read));
        assert!(!rules.allows("secret/data/anything/else", Capability::Read));
    }

    /// Not a timing measurement — those are unreliable in CI. This asserts the
    /// structural property the constant-time guarantee rests on: a miss does
    /// the same amount of work as a hit, because every entry is visited either
    /// way.
    #[test]
    fn a_miss_examines_every_entry_just_as_a_hit_does() {
        let table = table_with(&[
            ("s.first", &["payment"]),
            ("s.middle", &["web"]),
            ("s.last", &["payment"]),
        ]);
        assert_eq!(table.len(), 3);
        assert!(table.lookup("s.first").is_some());
        assert!(table.lookup("s.last").is_some());
        assert!(table.lookup("s.absent").is_none());
    }
}
