//! Named credential accounts (`[accounts.<name>]`).
//!
//! An account is one credential the user holds with a provider, named so a
//! model, a model provider, or a subagent role can point at it:
//!
//! ```toml
//! [accounts.ds-main]
//! kind = "api-key"
//! env_key = "DEEPSEEK_API_KEY"
//!
//! [accounts.ds-alt]
//! kind = "api-key"          # key stored by `bcode account add ds-alt`
//!
//! [model.deepseek-v4-pro]
//! account = "ds-main"
//! ```
//!
//! Several accounts are live in one process at once: the credential is resolved
//! per model at request time, so two subagents can run on two different keys
//! concurrently. That is the whole point of the table — nothing here switches a
//! global identity.
//!
//! Storage is `auth.json`, under the scope `account::<name>`, rather than a
//! second credential file. That file already has the atomic write, the
//! corrupt-recovery reader, the owner-only permissions and the cross-process
//! lock; a second store would have to grow all four again, and a second set of
//! bugs with them.
//!
//! A second, coarser scope lives here too: `provider::<id>`, one credential per
//! *provider* rather than per named account. `bcode login` writes here. It is
//! the zero-config path — a key stored for `deepseek` works for every DeepSeek
//! model with nothing in `config.toml` — and it sits below a named account in
//! the credential precedence (`resolve_credentials`), so `bcode account add`
//! remains how a second or third credential on the same provider is reached.

use std::path::Path;

use crate::agent::config::EnvKeys;

use super::model::{AuthMode, BcodeAuth};
use super::storage::{read_auth_json, read_auth_json_or_empty_recovering_corrupt, write_auth_json};

/// Scope prefix for a named account in `auth.json`.
///
/// Distinct from `bcode::` so an account can never collide with the session
/// scopes, whatever the user names it.
pub const ACCOUNT_SCOPE_PREFIX: &str = "account::";

/// Scope prefix for a provider-wide credential in `auth.json` (`bcode login`).
/// Distinct from [`ACCOUNT_SCOPE_PREFIX`] so a provider id and an account name
/// can never collide even if a user names an account after a provider.
pub const PROVIDER_SCOPE_PREFIX: &str = "provider::";

/// Longest account name accepted. Long enough for a descriptive name, short
/// enough that a scope key stays readable in `auth.json`.
const MAX_ACCOUNT_NAME: usize = 64;

/// What kind of credential an account holds.
///
/// Only `ApiKey` is implemented. `Oauth` is reserved for subscription sign-in:
/// it parses, so a config written against it keeps working, but it resolves no
/// credential and says so rather than serving whatever key happens to be
/// stored. Which provider such an account signs in to is named by `provider`,
/// against the catalog — provider names live in the catalog data file, not in
/// this enum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountKind {
    /// A provider API key, from `env_key` or from `bcode account add`.
    #[default]
    ApiKey,
    /// Reserved: interactive sign-in against the account's `provider`.
    Oauth,
}

impl AccountKind {
    /// Whether this kind can resolve a credential today.
    pub fn is_implemented(self) -> bool {
        matches!(self, Self::ApiKey)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api-key",
            Self::Oauth => "oauth",
        }
    }
}

/// One `[accounts.<name>]` table.
// `EnvKeys` is not `PartialEq`, so neither is this; nothing compares two
// account tables.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct AccountConfig {
    pub kind: AccountKind,
    /// Environment variable(s) holding this account's key. Checked before the
    /// stored key, so an account can be driven entirely from the environment
    /// with nothing written to disk.
    pub env_key: Option<EnvKeys>,
    /// Which catalog provider this account signs in to. Only meaningful for
    /// `kind = "oauth"`, which is not implemented yet.
    pub provider: Option<String>,
    /// Free-text label for `bcode account list`.
    pub description: Option<String>,
}

/// A model's reference to a named account, resolved by `resolve_model_list`.
///
/// Carries the config so the credential seam needs no access to the config
/// stack, and no key: the key is read at request time, so a rotated env var or
/// a re-run `bcode account add` takes effect without a restart.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AccountRef {
    pub name: String,
    #[serde(skip)]
    config: AccountConfig,
}

impl AccountRef {
    pub fn new(name: String, config: AccountConfig) -> Self {
        Self { name, config }
    }

    /// The name alone, for a ref revived from bytes (a persisted session) whose
    /// config has to be re-attached before it can resolve.
    pub fn unresolved(name: String) -> Self {
        Self {
            name,
            config: AccountConfig::default(),
        }
    }

    pub fn kind(&self) -> AccountKind {
        self.config.kind
    }

    /// This account's credential: `env_key` first, then the stored key.
    ///
    /// `None` when the account holds nothing yet, or when its kind is not one
    /// this build can resolve — never a partial or placeholder credential.
    pub fn credential(&self, bcode_home: &Path) -> Option<String> {
        if !self.config.kind.is_implemented() {
            tracing::warn!(
                account = %self.name,
                kind = %self.config.kind.as_str(),
                "account kind is not implemented yet; no credential resolved"
            );
            return None;
        }
        if let Some(key) = self
            .config
            .env_key
            .as_ref()
            .and_then(EnvKeys::resolve_value)
        {
            return Some(key);
        }
        read_account_key(bcode_home, &self.name)
    }
}

/// Whether `name` may be used as an account name.
///
/// An allowlist, because the name becomes a key in `auth.json`: a name carrying
/// `:` could otherwise be written to shadow a session scope.
pub fn is_valid_account_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_ACCOUNT_NAME
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !name.starts_with('.')
}

/// Whether `id` may be used as a provider id. Same allowlist as an account
/// name, for the same reason: it becomes a key in `auth.json`.
pub fn is_valid_provider_id(id: &str) -> bool {
    is_valid_account_name(id)
}

/// The `auth.json` scope holding `name`'s credential.
pub fn account_scope(name: &str) -> String {
    format!("{ACCOUNT_SCOPE_PREFIX}{name}")
}

/// The `auth.json` scope holding provider `id`'s credential.
pub fn provider_scope(id: &str) -> String {
    format!("{PROVIDER_SCOPE_PREFIX}{id}")
}

fn auth_json(bcode_home: &Path) -> std::path::PathBuf {
    bcode_home.join("auth.json")
}

/// Read a stored account key. `None` when the account has none.
pub fn read_account_key(bcode_home: &Path, name: &str) -> Option<String> {
    if !is_valid_account_name(name) {
        return None;
    }
    let store = read_auth_json(&auth_json(bcode_home)).ok()?;
    store
        .get(&account_scope(name))
        .map(|auth| auth.key.clone())
        .filter(|key| !key.trim().is_empty())
}

/// Read a stored provider key. `None` when the provider has none.
pub fn read_provider_key(bcode_home: &Path, id: &str) -> Option<String> {
    if !is_valid_provider_id(id) {
        return None;
    }
    let store = read_auth_json(&auth_json(bcode_home)).ok()?;
    store
        .get(&provider_scope(id))
        .map(|auth| auth.key.clone())
        .filter(|key| !key.trim().is_empty())
}

/// The credential `resolve_credentials` uses for its provider tier: the key
/// stored for `model_family` (a model's provider), if any.
///
/// `model_family` is `None` for a model the catalog or config didn't tag with
/// a provider — such a model has no provider tier to consult, only its own
/// key/account. Takes `bcode_home` explicitly, like [`AccountRef::credential`],
/// so it is unit-testable without the process-global cache in
/// `bcode_dirs::bcode_home()`.
pub fn provider_credential(model_family: Option<&str>, bcode_home: &Path) -> Option<String> {
    read_provider_key(bcode_home, model_family?)
}

/// Store `key` for `name`, replacing any previous credential.
pub fn store_account_key(bcode_home: &Path, name: &str, key: &str) -> std::io::Result<()> {
    store_scoped_key(bcode_home, &account_scope(name), key).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "invalid account name {name:?}: letters, digits, '_', '-' and '.', \
                 up to {MAX_ACCOUNT_NAME} characters, not starting with '.'"
            ),
        )
    })
}

/// Store `key` for provider `id`, replacing any previous credential.
/// This is what `bcode login` writes.
pub fn store_provider_key(bcode_home: &Path, id: &str, key: &str) -> std::io::Result<()> {
    store_scoped_key(bcode_home, &provider_scope(id), key).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "invalid provider id {id:?}: letters, digits, '_', '-' and '.', \
                 up to {MAX_ACCOUNT_NAME} characters, not starting with '.'"
            ),
        )
    })
}

/// Shared write path for both scopes: validate the scope's own name/id
/// (embedded in `scope`, checked by the caller before formatting it) is
/// unreachable here, so this validates the whole scope string is one of ours.
fn store_scoped_key(bcode_home: &Path, scope: &str, key: &str) -> std::io::Result<()> {
    let is_valid = scope
        .strip_prefix(ACCOUNT_SCOPE_PREFIX)
        .map(is_valid_account_name)
        .or_else(|| {
            scope
                .strip_prefix(PROVIDER_SCOPE_PREFIX)
                .map(is_valid_provider_id)
        })
        .unwrap_or(false);
    if !is_valid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid scope",
        ));
    }
    if key.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to store an empty credential",
        ));
    }
    let path = auth_json(bcode_home);
    let mut store = read_auth_json_or_empty_recovering_corrupt(&path)?;
    store.insert(
        scope.to_owned(),
        BcodeAuth {
            key: key.to_owned(),
            auth_mode: AuthMode::ApiKey,
            ..Default::default()
        },
    );
    write_auth_json(&path, &store)
}

/// Remove `name`'s stored credential. `Ok(false)` when there was none.
pub fn remove_account_key(bcode_home: &Path, name: &str) -> std::io::Result<bool> {
    remove_scoped_key(bcode_home, &account_scope(name))
}

/// Remove provider `id`'s stored credential. `Ok(false)` when there was none.
pub fn remove_provider_key(bcode_home: &Path, id: &str) -> std::io::Result<bool> {
    remove_scoped_key(bcode_home, &provider_scope(id))
}

fn remove_scoped_key(bcode_home: &Path, scope: &str) -> std::io::Result<bool> {
    let path = auth_json(bcode_home);
    let Ok(mut store) = read_auth_json(&path) else {
        return Ok(false);
    };
    if store.remove(scope).is_none() {
        return Ok(false);
    }
    write_auth_json(&path, &store)?;
    Ok(true)
}

/// The names of every account with a stored credential, sorted.
///
/// An account that resolves only from `env_key` has nothing here, which is
/// why `bcode account list` reads the config as well.
pub fn stored_account_names(bcode_home: &Path) -> Vec<String> {
    stored_scoped_names(bcode_home, ACCOUNT_SCOPE_PREFIX)
}

/// The ids of every provider with a stored credential, sorted.
pub fn stored_provider_ids(bcode_home: &Path) -> Vec<String> {
    stored_scoped_names(bcode_home, PROVIDER_SCOPE_PREFIX)
}

fn stored_scoped_names(bcode_home: &Path, prefix: &str) -> Vec<String> {
    let Ok(store) = read_auth_json(&auth_json(bcode_home)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = store
        .keys()
        .filter_map(|scope| scope.strip_prefix(prefix))
        .map(str::to_owned)
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn names_are_an_allowlist_so_a_scope_cannot_be_forged() {
        assert!(is_valid_account_name("ds-main"));
        assert!(is_valid_account_name("work.2"));
        assert!(!is_valid_account_name(""));
        assert!(!is_valid_account_name(".hidden"));
        assert!(!is_valid_account_name("bcode::api_key"));
        assert!(!is_valid_account_name("a/b"));
        assert!(!is_valid_account_name("with space"));
        assert!(!is_valid_account_name(&"x".repeat(MAX_ACCOUNT_NAME + 1)));
    }

    #[test]
    fn a_stored_key_round_trips_and_removes() {
        let dir = home();
        assert_eq!(read_account_key(dir.path(), "ds-main"), None);
        store_account_key(dir.path(), "ds-main", "sk-one").expect("store");
        assert_eq!(
            read_account_key(dir.path(), "ds-main").as_deref(),
            Some("sk-one")
        );
        store_account_key(dir.path(), "ds-alt", "sk-two").expect("store");
        assert_eq!(
            stored_account_names(dir.path()),
            vec!["ds-alt".to_owned(), "ds-main".to_owned()]
        );
        assert!(remove_account_key(dir.path(), "ds-main").expect("remove"));
        assert_eq!(read_account_key(dir.path(), "ds-main"), None);
        // The other account is untouched: removal is per-scope, not per-file.
        assert_eq!(
            read_account_key(dir.path(), "ds-alt").as_deref(),
            Some("sk-two")
        );
        assert!(!remove_account_key(dir.path(), "ds-main").expect("remove"));
    }

    #[test]
    fn an_empty_or_invalid_name_is_refused_rather_than_written() {
        let dir = home();
        for (name, key) in [("ds-main", "  "), ("bad name", "sk"), ("", "sk")] {
            let err = store_account_key(dir.path(), name, key).expect_err("must refuse");
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        }
        assert!(stored_account_names(dir.path()).is_empty());
    }

    #[test]
    fn a_session_scope_survives_an_account_write() {
        let dir = home();
        super::super::storage::store_api_key(dir.path(), "session-key").expect("store api key");
        store_account_key(dir.path(), "ds-main", "sk-one").expect("store account");
        assert_eq!(
            super::super::storage::read_api_key(dir.path()).as_deref(),
            Some("session-key"),
            "an account write must not disturb the session credential"
        );
        assert_eq!(stored_account_names(dir.path()), vec!["ds-main".to_owned()]);
    }

    #[test]
    fn env_key_beats_the_stored_key_so_rotation_needs_no_write() {
        let dir = home();
        store_account_key(dir.path(), "ds-main", "stored").expect("store");
        let account = AccountRef::new(
            "ds-main".to_owned(),
            AccountConfig {
                kind: AccountKind::ApiKey,
                env_key: Some(EnvKeys::single("BCODE_TEST_ACCOUNT_KEY")),
                ..AccountConfig::default()
            },
        );
        assert_eq!(
            account.credential(dir.path()).as_deref(),
            Some("stored"),
            "unset env var falls through to the stored key"
        );
        let _guard = bcode_test_support::EnvGuard::set("BCODE_TEST_ACCOUNT_KEY", "from-env");
        assert_eq!(account.credential(dir.path()).as_deref(), Some("from-env"));
    }

    #[test]
    fn a_reserved_kind_resolves_to_nothing_rather_than_the_stored_key() {
        let dir = home();
        store_account_key(dir.path(), "work", "stored").expect("store");
        let account = AccountRef::new(
            "work".to_owned(),
            AccountConfig {
                kind: AccountKind::Oauth,
                ..AccountConfig::default()
            },
        );
        assert_eq!(
            account.credential(dir.path()),
            None,
            "a reserved kind must not silently serve an API key"
        );
    }

    #[test]
    fn a_provider_key_round_trips_and_removes_independently_of_accounts() {
        let dir = home();
        assert_eq!(provider_credential(Some("deepseek"), dir.path()), None);
        store_provider_key(dir.path(), "deepseek", "sk-provider").expect("store");
        assert_eq!(
            provider_credential(Some("deepseek"), dir.path()).as_deref(),
            Some("sk-provider")
        );
        assert_eq!(stored_provider_ids(dir.path()), vec!["deepseek".to_owned()]);

        // An account of the same name occupies a distinct scope.
        store_account_key(dir.path(), "deepseek", "sk-account").expect("store");
        assert_eq!(
            provider_credential(Some("deepseek"), dir.path()).as_deref(),
            Some("sk-provider"),
            "an account named the same as a provider must not shadow it"
        );
        assert_eq!(
            read_account_key(dir.path(), "deepseek").as_deref(),
            Some("sk-account")
        );

        assert!(remove_provider_key(dir.path(), "deepseek").expect("remove"));
        assert_eq!(provider_credential(Some("deepseek"), dir.path()), None);
        assert_eq!(
            read_account_key(dir.path(), "deepseek").as_deref(),
            Some("sk-account"),
            "removing the provider credential must not touch the account"
        );
        assert!(!remove_provider_key(dir.path(), "deepseek").expect("remove"));
    }

    #[test]
    fn a_model_with_no_provider_has_no_provider_tier_to_consult() {
        let dir = home();
        store_provider_key(dir.path(), "deepseek", "sk-provider").expect("store");
        assert_eq!(provider_credential(None, dir.path()), None);
    }

    #[test]
    fn an_empty_or_invalid_provider_id_is_refused_rather_than_written() {
        let dir = home();
        for (id, key) in [("deepseek", "  "), ("bad id", "sk"), ("", "sk")] {
            let err = store_provider_key(dir.path(), id, key).expect_err("must refuse");
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        }
        assert!(stored_provider_ids(dir.path()).is_empty());
    }

    /// The acceptance test for the whole tier: one key, stored once for
    /// `deepseek`, resolves for every DeepSeek row in the real embedded
    /// catalog -- and for none of the other providers' rows -- with no
    /// `config.toml` involved at all.
    #[test]
    fn one_provider_key_resolves_for_every_model_that_shares_it() {
        #[derive(serde::Deserialize)]
        struct Row {
            model_family: Option<String>,
        }
        #[derive(serde::Deserialize)]
        struct Catalog {
            models: Vec<Row>,
        }
        let catalog: Catalog =
            serde_json::from_str(bcode_models::DEFAULT_MODELS_JSON).expect("catalog parses");
        let families: std::collections::BTreeSet<String> = catalog
            .models
            .into_iter()
            .filter_map(|m| m.model_family)
            .collect();
        assert!(
            families.contains("deepseek") && families.len() > 1,
            "fixture assumption: the catalog has DeepSeek and at least one other provider"
        );

        let dir = home();
        store_provider_key(dir.path(), "deepseek", "sk-ds").expect("store");
        for family in &families {
            let resolved = provider_credential(Some(family), dir.path());
            if family == "deepseek" {
                assert_eq!(resolved.as_deref(), Some("sk-ds"));
            } else {
                assert_eq!(
                    resolved, None,
                    "a {family} model must not see DeepSeek's key"
                );
            }
        }
    }
}
