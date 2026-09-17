//! Authorization for a read-only resolver (ADR-0015 D8).
//!
//! Two halves, and nothing else: [`token`] turns the `X-Vault-Token` an app
//! presents into the set of rules it carries, and [`matcher`] decides whether
//! those rules permit one path and one capability. Both are pure functions over
//! data that came out of the sealed file — there is no store, no lease, and
//! nothing that expires on a clock.

pub mod matcher;
pub mod token;

pub use matcher::{Capability, CompiledRule, RuleSet};
pub use token::{Grant, TOKEN_KEY_LEN, TokenError, TokenKey, TokenTable};
