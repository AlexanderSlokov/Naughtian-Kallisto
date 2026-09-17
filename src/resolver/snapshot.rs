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
use core_crypto::{Contents, PolicyRule};

pub struct Snapshot {
    /// The sealed file's content version. Also what `sys/health` reports, so an
    /// operator can ask three machines whether they agree (ADR-0015 D4).
    pub version: u64,
    pub etag: Option<String>,
    pub loaded_at: SystemTime,
    /// Path (no `secret/data/` prefix) to its KV-v2 data object, already
    /// rendered as JSON. The read path concatenates this into the response and
    /// never parses anything.
    ///
    /// ADR-0015 D13's in-memory barrier replaces the value here with a
    /// re-sealed blob; this is the one seam that changes when that lands.
    secrets: HashMap<String, String>,
    policies: HashMap<String, Vec<PolicyRule>>,
    tokens: HashMap<String, Vec<String>>,
    /// Every path, sorted, so a LIST can find a prefix range by binary search
    /// rather than scanning.
    sorted_paths: Vec<String>,
}

impl Snapshot {
    pub fn build(contents: Contents, etag: Option<String>) -> Self {
        let Contents {
            version,
            secrets,
            policies,
            tokens,
        } = contents;

        let mut sorted_paths: Vec<String> = secrets.keys().cloned().collect();
        sorted_paths.sort();

        Self {
            version,
            etag,
            loaded_at: SystemTime::now(),
            secrets: render_once(secrets),
            policies: policies.into_iter().collect(),
            tokens: tokens.into_iter().collect(),
            sorted_paths,
        }
    }

    pub fn secret(&self, path: &str) -> Option<&str> {
        self.secrets.get(path).map(String::as_str)
    }

    pub fn policies_for(&self, token_hash: &str) -> Option<&[String]> {
        self.tokens.get(token_hash).map(Vec::as_slice)
    }

    pub fn rules(&self, policy: &str) -> Option<&[PolicyRule]> {
        self.policies.get(policy).map(Vec::as_slice)
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
            .field("policies", &self.policies.len())
            .field("tokens", &self.tokens.len())
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
    use super::*;

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
            tokens: BTreeMap::from([("hash1".to_string(), vec!["db".to_string()])]),
        }
    }

    #[test]
    fn secrets_are_rendered_once_at_build_time() {
        let snap = Snapshot::build(contents(), Some("etag-1".into()));
        assert_eq!(snap.version, 12);
        assert_eq!(snap.secret("app/db"), Some(r#"{"user":"admin"}"#));
        assert_eq!(snap.secret("nope"), None);
    }

    /// Vault's LIST returns immediate children only, with directories marked by
    /// a trailing slash and listed once however many secrets sit beneath them.
    #[test]
    fn children_collapse_nested_paths_into_one_directory_entry() {
        let snap = Snapshot::build(contents(), None);
        assert_eq!(snap.children("app/"), vec!["db", "sub/", "web"]);
        assert_eq!(snap.children("app/sub/"), vec!["deep"]);
        assert_eq!(snap.children(""), vec!["app/", "other"]);
        assert!(snap.children("missing/").is_empty());
    }

    #[test]
    fn policies_and_tokens_are_reachable() {
        let snap = Snapshot::build(contents(), None);
        assert_eq!(snap.policies_for("hash1"), Some(&["db".to_string()][..]));
        assert_eq!(snap.policies_for("unknown"), None);
        assert_eq!(snap.rules("db").unwrap()[0].path, "secret/data/app/*");
    }

    #[test]
    fn an_empty_slot_reads_as_not_ready() {
        let slot = SnapshotSlot::empty();
        assert!(slot.load().is_none());
        assert_eq!(slot.version(), None);

        slot.store(Snapshot::build(contents(), None));
        assert_eq!(slot.version(), Some(12));
        assert_eq!(slot.load().unwrap().secret_count(), 4);
    }

    #[test]
    fn debug_shows_counts_not_contents() {
        let rendered = format!("{:?}", Snapshot::build(contents(), None));
        assert!(
            !rendered.contains("admin"),
            "Debug leaked a secret: {rendered}"
        );
        assert!(
            !rendered.contains("app/db"),
            "Debug leaked a path: {rendered}"
        );
        assert!(rendered.contains("REDACTED"));
    }
}
