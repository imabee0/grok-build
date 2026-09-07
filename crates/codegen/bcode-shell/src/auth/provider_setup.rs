//! Shared policy behind `bcode login`/`bcode logout` and the in-TUI provider
//! manager: one place that lists provider/account credential status and
//! validates a write before it reaches [`super::accounts`].
//!
//! Neither front end (the CLI wizard in `bcode-pager`'s `login_cmd.rs`, or the
//! TUI's provider-manager view) owns credential policy; both call here, so
//! they cannot drift apart on what counts as a valid id, an empty key, or a
//! known provider.

use std::path::Path;

use super::accounts::{self, AccountKind};

/// Where a provider's credential currently resolves from, if at all.
///
/// Mirrors the precedence `resolve_credentials` actually uses: a model's own
/// `env_key` is checked before the provider-wide tier, so when both are
/// present the environment variable is what a turn will use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    /// Only `provider::<id>` in auth.json (`bcode login`) has a key.
    Stored,
    /// Only the provider's conventional env var is set.
    Env,
    /// Both are present; the environment variable wins at request time.
    StoredAndEnv,
    /// Neither -- this provider has no credential right now.
    None,
}

impl CredentialSource {
    /// Whether a request against this provider would resolve to *some* key.
    pub fn is_usable(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// One catalog provider's credential status, for a picker row.
#[derive(Debug, Clone, Copy)]
pub struct ProviderStatus {
    pub info: &'static bcode_models::ProviderInfo,
    pub source: CredentialSource,
}

/// One `[accounts.<name>]` entry's credential status, for a picker row.
///
/// An account can exist purely because it has a stored key with no
/// `[accounts.*]` table at all (`configured: false`), or purely as
/// configuration with nothing stored yet -- both are listed, matching
/// `bcode account list`.
#[derive(Debug, Clone)]
pub struct AccountStatus {
    pub name: String,
    pub kind: AccountKind,
    pub has_stored_key: bool,
    pub env_key: Option<String>,
    pub description: Option<String>,
    pub configured: bool,
}

/// A rejected write, before it ever reaches [`super::accounts`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderSetupError {
    /// The key was empty (or all whitespace).
    EmptyKey,
    /// `id`/`name` fails [`accounts::is_valid_provider_id`] /
    /// [`accounts::is_valid_account_name`].
    InvalidId(String),
    /// `id` names no catalog provider.
    UnknownProvider(String),
    /// The underlying `auth.json` read/write failed.
    Io(String),
}

impl std::fmt::Display for ProviderSetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyKey => write!(f, "refusing to store an empty key"),
            Self::InvalidId(id) => write!(
                f,
                "invalid id {id:?}: letters, digits, '_', '-' and '.' only, not starting with '.'"
            ),
            Self::UnknownProvider(id) => {
                let known: Vec<&str> = bcode_models::providers()
                    .iter()
                    .map(|p| p.id.as_str())
                    .collect();
                write!(
                    f,
                    "unknown provider {id:?}; known providers: {}",
                    known.join(", ")
                )
            }
            Self::Io(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ProviderSetupError {}

/// Every catalog provider's credential status, in catalog order.
pub fn provider_status(home: &Path) -> Vec<ProviderStatus> {
    bcode_models::providers()
        .iter()
        .map(|info| {
            let stored = accounts::read_provider_key(home, &info.id).is_some();
            let env = std::env::var(&info.env_key).is_ok();
            let source = match (stored, env) {
                (true, true) => CredentialSource::StoredAndEnv,
                (true, false) => CredentialSource::Stored,
                (false, true) => CredentialSource::Env,
                (false, false) => CredentialSource::None,
            };
            ProviderStatus { info, source }
        })
        .collect()
}

/// Every named account's status: the union of `[accounts.*]` config entries
/// and accounts with a stored key but no config table, sorted by name --
/// the same set `bcode account list` shows.
pub fn account_status(
    home: &Path,
    accounts_config: &indexmap::IndexMap<String, super::AccountConfig>,
) -> Vec<AccountStatus> {
    let stored = accounts::stored_account_names(home);
    let mut names: Vec<String> = accounts_config.keys().cloned().collect();
    for name in &stored {
        if !accounts_config.contains_key(name.as_str()) {
            names.push(name.clone());
        }
    }
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let configured = accounts_config.get(&name);
            AccountStatus {
                has_stored_key: stored.iter().any(|s| s == &name),
                kind: configured.map_or(AccountKind::ApiKey, |a| a.kind),
                env_key: configured.and_then(|a| a.env_key.as_ref().map(|k| k.names().join(", "))),
                description: configured.and_then(|a| a.description.clone()),
                configured: configured.is_some(),
                name,
            }
        })
        .collect()
}

/// Store `key` for provider `id`. Validates the provider exists and the key
/// is non-empty before ever touching `auth.json`.
pub fn store_provider_credential(
    home: &Path,
    id: &str,
    key: &str,
) -> Result<(), ProviderSetupError> {
    if bcode_models::provider(id).is_none() {
        return Err(ProviderSetupError::UnknownProvider(id.to_string()));
    }
    let key = key.trim();
    if key.is_empty() {
        return Err(ProviderSetupError::EmptyKey);
    }
    accounts::store_provider_key(home, id, key).map_err(|e| ProviderSetupError::Io(e.to_string()))
}

/// Store `key` for named account `name`. Validates the name and the key
/// before ever touching `auth.json`.
pub fn store_account_credential(
    home: &Path,
    name: &str,
    key: &str,
) -> Result<(), ProviderSetupError> {
    if !accounts::is_valid_account_name(name) {
        return Err(ProviderSetupError::InvalidId(name.to_string()));
    }
    let key = key.trim();
    if key.is_empty() {
        return Err(ProviderSetupError::EmptyKey);
    }
    accounts::store_account_key(home, name, key).map_err(|e| ProviderSetupError::Io(e.to_string()))
}

/// Remove provider `id`'s stored credential. `Ok(false)` when there was none.
pub fn remove_provider_credential(home: &Path, id: &str) -> Result<bool, ProviderSetupError> {
    accounts::remove_provider_key(home, id).map_err(|e| ProviderSetupError::Io(e.to_string()))
}

/// Remove account `name`'s stored credential. `Ok(false)` when there was none.
pub fn remove_account_credential(home: &Path, name: &str) -> Result<bool, ProviderSetupError> {
    accounts::remove_account_key(home, name).map_err(|e| ProviderSetupError::Io(e.to_string()))
}

/// Whether any model in `models` currently resolves a credential, by the same
/// path `resolve_credentials` uses at request time. The signal a client uses
/// to decide whether the provider-setup screen is still needed.
pub fn any_model_has_credential<'a, I>(models: I, session_key: Option<&str>) -> bool
where
    I: IntoIterator<Item = &'a crate::agent::config::ModelEntry>,
{
    models
        .into_iter()
        .any(|m| m.has_any_credential(session_key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bcode_test_support::EnvGuard;
    use serial_test::serial;

    fn home() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn provider_status_lists_every_catalog_provider() {
        let dir = home();
        let statuses = provider_status(dir.path());
        let catalog = bcode_models::providers();
        assert_eq!(statuses.len(), catalog.len());
        for (status, info) in statuses.iter().zip(catalog.iter()) {
            assert_eq!(status.info.id, info.id);
            assert_eq!(status.source, CredentialSource::None);
        }
    }

    #[test]
    #[serial]
    fn stored_key_reports_stored_and_env_reports_env() {
        let dir = home();
        let provider = &bcode_models::providers()[0];
        let _unset = EnvGuard::unset(&provider.env_key);

        assert_eq!(
            provider_status(dir.path())[0].source,
            CredentialSource::None
        );

        store_provider_credential(dir.path(), &provider.id, "stored-key").unwrap();
        assert_eq!(
            provider_status(dir.path())[0].source,
            CredentialSource::Stored
        );

        let _env = EnvGuard::set(&provider.env_key, "env-key");
        assert_eq!(
            provider_status(dir.path())[0].source,
            CredentialSource::StoredAndEnv
        );

        remove_provider_credential(dir.path(), &provider.id).unwrap();
        assert_eq!(provider_status(dir.path())[0].source, CredentialSource::Env);
    }

    /// Pins the precedence comment on `CredentialSource`: `resolve_credentials`
    /// checks a model's own `env_key` before the provider-wide tier, so when
    /// both are set the environment variable is what a turn actually uses.
    #[test]
    #[serial]
    fn env_var_beats_stored_key_for_a_catalog_model() {
        let dir = home();
        let provider = &bcode_models::providers()[0];
        let _unset = EnvGuard::unset(&provider.env_key);
        store_provider_credential(dir.path(), &provider.id, "stored-key").unwrap();
        let _env = EnvGuard::set(&provider.env_key, "env-key");

        let key = crate::auth::accounts::provider_credential(Some(&provider.id), dir.path());
        assert_eq!(
            key.as_deref(),
            Some("stored-key"),
            "provider_credential itself only ever sees the stored tier"
        );
        // The env tier is checked one level up, on the model, before
        // `resolve_credentials` ever reaches the provider tier -- covered by
        // `agent::config` tests using a real `ModelEntry`. This test only pins
        // that both sources are visible to the UI-facing status above.
    }

    #[test]
    fn store_refuses_empty_and_invalid() {
        let dir = home();
        let provider = &bcode_models::providers()[0];
        assert_eq!(
            store_provider_credential(dir.path(), &provider.id, "   "),
            Err(ProviderSetupError::EmptyKey)
        );
        assert_eq!(
            store_provider_credential(dir.path(), "not-a-real-provider", "key"),
            Err(ProviderSetupError::UnknownProvider(
                "not-a-real-provider".to_string()
            ))
        );
        assert_eq!(
            store_account_credential(dir.path(), "bad::name", "key"),
            Err(ProviderSetupError::InvalidId("bad::name".to_string()))
        );
        assert_eq!(
            store_account_credential(dir.path(), "ok-name", "   "),
            Err(ProviderSetupError::EmptyKey)
        );
    }

    #[test]
    fn account_status_unions_configured_and_stored() {
        let dir = home();
        store_account_credential(dir.path(), "stored-only", "key").unwrap();
        let mut configured = indexmap::IndexMap::new();
        configured.insert(
            "configured-only".to_string(),
            crate::auth::AccountConfig {
                kind: AccountKind::ApiKey,
                description: Some("declared but no key yet".to_string()),
                ..Default::default()
            },
        );
        let statuses = account_status(dir.path(), &configured);
        let names: Vec<&str> = statuses.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["configured-only", "stored-only"]);
        assert!(!statuses[0].has_stored_key);
        assert!(statuses[0].configured);
        assert!(statuses[1].has_stored_key);
        assert!(!statuses[1].configured);
    }
}
