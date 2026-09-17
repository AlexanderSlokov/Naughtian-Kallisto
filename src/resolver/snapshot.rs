//! The table the HTTP layer answers out of.
//!
//! Built once per file change and swapped in whole (ADR-0015 D10 step 3). A
//! request never sees a half-updated table, and a bad file leaves the previous
//! one exactly where it was.
//!
//! ADR-0015 D11 sized this deliberately: a few dozen secrets, read almost
//! exclusively, so a plain `HashMap` behind `arc-swap` is enough. The project's
//! own benchmark put the time in the HTTP layer, not the lookup.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::SystemTime,
};

use arc_swap::ArcSwapOption;
use core_crypto::Contents;
use policy_engine::{Capability, Grant, TokenError, TokenTable};

pub struct Snapshot {
    /// The sealed file's content version. Also what `sys/health` reports, so an
    /// operator can ask three machines whether they agree (ADR-0015 D4).
    pub version: u64,
    pub etag: Option<String>,
    pub loaded_at: SystemTime,
    /// Rendered here rather than per response: every KV read carries this
    /// string, and formatting a timestamp eight thousand times a second to
    /// produce the same eight thousand identical strings is work the hot path
    /// does not need to do (ADR-0016 QĐ-3).
    loaded_at_rfc3339: String,
    /// Path (no `secret/data/` prefix) to its KV-v2 data object, already
    /// rendered as JSON. The read path concatenates this into the response and
    /// never parses anything.
    ///
    /// ADR-0015 D13's in-memory barrier replaces the value here with a
    /// re-sealed blob; this is the one seam that changes when that lands.
    secrets: HashMap<String, String>,
    /// `None` means the file carries no token key, which ADR-0015 D8 defines as
    /// the sidecar deployment: one app, its own file, and the bucket credential
    /// is the boundary. Every read is then permitted, and [`Self::enforces`]
    /// reports that so an operator can see it on `sys/health` rather than
    /// discovering it.
    tokens: Option<TokenTable>,
    /// Every path, sorted, so a LIST can find a prefix range by binary search
    /// rather than scanning.
    sorted_paths: Vec<String>,
}

/// A file that parsed and authenticated but cannot be turned into a servable
/// table. Reported the same way a forged file is: the previous table stays.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error(transparent)]
    Tokens(#[from] TokenError),
}

impl Snapshot {
    pub fn build(contents: Contents, etag: Option<String>) -> Result<Self, SnapshotError> {
        let Contents {
            version,
            secrets,
            policies,
            tokens,
            token_key,
        } = contents;

        // A table of tokens with no key to check them against would refuse
        // everyone; no key and no tokens is the sidecar case and is fine.
        // Serving the first as though it were the second would mean serving
        // with authorization quietly switched off.
        let tokens = match (&token_key, tokens.is_empty()) {
            (Some(key_hex), _) => Some(TokenTable::build(key_hex, &tokens, &policies)?),
            (None, true) => None,
            (None, false) => {
                return Err(TokenError::KeyMissing {
                    tokens: tokens.len(),
                }
                .into());
            }
        };

        let mut sorted_paths: Vec<String> = secrets.keys().cloned().collect();
        sorted_paths.sort();

        let loaded_at = SystemTime::now();
        Ok(Self {
            version,
            etag,
            loaded_at,
            loaded_at_rfc3339: crate::server::time_format::rfc3339_from_system_time(loaded_at),
            secrets: render_once(secrets),
            tokens,
            sorted_paths,
        })
    }

    pub fn loaded_at_rfc3339(&self) -> &str {
        &self.loaded_at_rfc3339
    }

    pub fn secret(&self, path: &str) -> Option<&str> {
        self.secrets.get(path).map(String::as_str)
    }

    /// Whether this file asks for tokens at all (ADR-0015 D8).
    pub fn enforces(&self) -> bool {
        self.tokens.is_some()
    }

    /// Policy names a token referred to that the file does not define. Empty
    /// unless somebody mistyped one.
    pub fn unknown_policies(&self) -> &[String] {
        self.tokens
            .as_ref()
            .map_or(&[], |table| table.unknown_policies())
    }

    pub fn token_count(&self) -> usize {
        self.tokens.as_ref().map_or(0, TokenTable::len)
    }

    /// What the presented token carries, if the file enforces at all.
    pub fn grant(&self, presented: Option<&str>) -> Option<Grant<'_>> {
        let table = self.tokens.as_ref()?;
        table.lookup(presented?)
    }

    /// The one question the HTTP layer asks.
    ///
    /// `path` is the full Vault-style path with the mount and the `data/` or
    /// `metadata/` segment already in it — `secret/data/payment/db` — because
    /// that is what a policy is written against (ADR-0015 D8).
    ///
    /// A file with no token key permits everything: that is the sidecar
    /// deployment, where the bucket credential already decided who may read
    /// this file at all. Anything else is default deny, including a request
    /// that carried no token.
    pub fn permits(&self, presented: Option<&str>, path: &str, capability: Capability) -> bool {
        let Some(table) = &self.tokens else {
            return true;
        };
        let Some(presented) = presented else {
            return false;
        };
        table
            .lookup(presented)
            .is_some_and(|grant| grant.rules.allows(path, capability))
    }

    pub fn secret_count(&self) -> usize {
        self.secrets.len()
    }

    /// Immediate children of `prefix`, in Vault's LIST shape: a nested path
    /// contributes its next segment with a trailing slash, once.
    ///
    /// `prefix` is either empty or ends in `/`.
    pub fn children(&self, prefix: &str) -> Vec<String> {
        let start = self.sorted_paths.partition_point(|p| p.as_str() < prefix);
        let mut seen = Vec::new();

        for path in &self.sorted_paths[start..] {
            let Some(rest) = path.strip_prefix(prefix) else {
                break; // sorted, so the prefix range has ended
            };
            let child = match rest.find('/') {
                Some(slash) => format!("{}/", &rest[..slash]),
                None => rest.to_string(),
            };
            if seen.last() != Some(&child) {
                seen.push(child);
            }
        }
        seen
    }
}

/// Serialising each secret once, here, is what keeps `serde` off the read path.
fn render_once(secrets: BTreeMap<String, serde_json::Value>) -> HashMap<String, String> {
    secrets
        .into_iter()
        .map(|(path, value)| (path, value.to_string()))
        .collect()
}

/// Counts and version, never contents — this reaches logs through `{:?}`.
impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Snapshot")
            .field("version", &self.version)
            .field("etag", &self.etag)
            .field(
                "secrets",
                &format_args!("<{} REDACTED>", self.secrets.len()),
            )
            .field("enforces", &self.enforces())
            .field("tokens", &self.token_count())
            .finish()
    }
}

/// The swap point. `None` means nothing has loaded yet, which the HTTP layer
/// reports as Vault's sealed state — a 503, not an empty 200 (ADR-0015 D14).
pub struct SnapshotSlot(ArcSwapOption<Snapshot>);

impl Default for SnapshotSlot {
    fn default() -> Self {
        Self::empty()
    }
}

impl SnapshotSlot {
    pub fn empty() -> Self {
        Self(ArcSwapOption::empty())
    }

    pub fn load(&self) -> Option<Arc<Snapshot>> {
        self.0.load_full()
    }

    pub fn store(&self, snapshot: Snapshot) {
        self.0.store(Some(Arc::new(snapshot)));
    }

    /// The version currently served, if any. Feeds both `sys/health` and the
    /// anti-rollback check on the next poll.
    pub fn version(&self) -> Option<u64> {
        self.0.load().as_ref().map(|s| s.version)
    }
}

#[cfg(test)]
mod tests {
    use core_crypto::PolicyRule;
    use policy_engine::TokenKey;

    use super::*;

    fn token_key() -> TokenKey {
        TokenKey::from_bytes([9u8; 32])
    }

    fn contents() -> Contents {
        Contents {
            version: 12,
            secrets: BTreeMap::from([
                ("app/db".to_string(), serde_json::json!({"user": "admin"})),
                (
                    "app/web".to_string(),
                    serde_json::json!({"url": "http://x"}),
                ),
                ("app/sub/deep".to_string(), serde_json::json!({"k": "v"})),
                ("other".to_string(), serde_json::json!({"k": "v"})),
            ]),
            policies: BTreeMap::from([(
                "db".to_string(),
                vec![PolicyRule {
                    path: "secret/data/app/*".to_string(),
                    capabilities: vec!["read".to_string()],
                }],
            )]),
            tokens: BTreeMap::from([(token_key().hash_hex("s.apptoken"), vec!["db".to_string()])]),
            token_key: Some(token_key().expose_as_hex()),
        }
    }

    /// The sidecar shape of ADR-0015 D8: no token key, no token table.
    fn unguarded() -> Contents {
        Contents {
            tokens: BTreeMap::new(),
            token_key: None,
            ..contents()
        }
    }

    fn build(contents: Contents) -> Snapshot {
        Snapshot::build(contents, None).unwrap()
    }

    #[test]
    fn secrets_are_rendered_once_at_build_time() {
        let snap = Snapshot::build(contents(), Some("etag-1".into())).unwrap();
        assert_eq!(snap.version, 12);
        assert_eq!(snap.secret("app/db"), Some(r#"{"user":"admin"}"#));
        assert_eq!(snap.secret("nope"), None);
    }

    /// Vault's LIST returns immediate children only, with directories marked by
    /// a trailing slash and listed once however many secrets sit beneath them.
    #[test]
    fn children_collapse_nested_paths_into_one_directory_entry() {
        let snap = build(contents());
        assert_eq!(snap.children("app/"), vec!["db", "sub/", "web"]);
        assert_eq!(snap.children("app/sub/"), vec!["deep"]);
        assert_eq!(snap.children(""), vec!["app/", "other"]);
        assert!(snap.children("missing/").is_empty());
    }

    #[test]
    fn a_token_reaches_the_rules_of_its_policies() {
        let snap = build(contents());
        assert!(snap.enforces());
        assert_eq!(snap.token_count(), 1);
        assert!(snap.unknown_policies().is_empty());

        let token = Some("s.apptoken");
        assert!(snap.permits(token, "secret/data/app/db", Capability::Read));
        assert!(!snap.permits(token, "secret/data/other", Capability::Read));
        assert_eq!(snap.grant(token).unwrap().policies, &["db".to_string()]);
    }

    #[test]
    fn a_wrong_token_and_a_missing_token_are_both_refused() {
        let snap = build(contents());
        assert!(!snap.permits(Some("s.wrong"), "secret/data/app/db", Capability::Read));
        assert!(!snap.permits(None, "secret/data/app/db", Capability::Read));
        assert!(snap.grant(Some("s.wrong")).is_none());
    }

    /// ADR-0015 D8's sidecar case: no token table means the bucket credential
    /// was the boundary, and every read is permitted.
    #[test]
    fn a_file_without_a_token_table_permits_everything() {
        let snap = build(unguarded());
        assert!(!snap.enforces());
        assert!(snap.permits(None, "secret/data/app/db", Capability::Read));
        assert!(snap.permits(Some("anything"), "secret/data/other", Capability::List));
    }

    /// Tokens with no key to check them against would authenticate nobody. A
    /// file in that shape is refused rather than served with authorization
    /// silently off.
    #[test]
    fn tokens_without_a_key_are_refused_outright() {
        let broken = Contents {
            token_key: None,
            ..contents()
        };
        let err = Snapshot::build(broken, None).unwrap_err();
        assert!(matches!(
            err,
            SnapshotError::Tokens(TokenError::KeyMissing { tokens: 1 })
        ));
    }

    #[test]
    fn an_empty_slot_reads_as_not_ready() {
        let slot = SnapshotSlot::empty();
        assert!(slot.load().is_none());
        assert_eq!(slot.version(), None);

        slot.store(build(contents()));
        assert_eq!(slot.version(), Some(12));
        assert_eq!(slot.load().unwrap().secret_count(), 4);
    }

    #[test]
    fn debug_shows_counts_not_contents() {
        let rendered = format!("{:?}", build(contents()));
        assert!(
            !rendered.contains("admin"),
            "Debug leaked a secret: {rendered}"
        );
        assert!(
            !rendered.contains("app/db"),
            "Debug leaked a path: {rendered}"
        );
        assert!(
            !rendered.contains("s.apptoken"),
            "Debug leaked a token: {rendered}"
        );
        assert!(rendered.contains("REDACTED"));
    }
}
