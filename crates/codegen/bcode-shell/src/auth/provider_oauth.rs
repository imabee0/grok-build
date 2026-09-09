//! Background refresh for a catalog provider's own OAuth credential
//! (`provider::<id>` in `auth.json`, `auth_mode = Oidc`).
//!
//! Mirrors [`super::AuthProviderRef`]'s pre-turn mint/refresh contract for
//! `[auth_provider.*]` commands, with one deliberate difference: the source
//! of truth here is disk (`auth.json`), not memory. A subscription sign-in
//! must survive a process restart, where a minted command token deliberately
//! must not (see that module's doc comment).
//!
//! [`ProviderOAuthRef::cached_token`] is the sync, disk-reading fallback
//! `resolve_credentials` calls, matching the cost the plain provider-key tier
//! already pays every call. [`ProviderOAuthRef::ensure_fresh_token`] and
//! [`ProviderOAuthRef::recover_rejected_token`] are the async pre-turn and
//! 401 arms `sampler_turn.rs` calls, exactly like `AuthProviderRef`'s.
//!
//! Every method takes `bcode_home` explicitly rather than reading the
//! process-global `bcode_dirs::bcode_home()` itself, matching
//! [`super::AccountRef::credential`]'s convention -- so this is unit-testable
//! against a temp dir, and callers already resolving `bcode_home()` once
//! (`resolve_credentials`, the pre-turn arms) don't pay for it twice.

use std::path::Path;

use super::accounts;
use super::model::{AuthMode, BcodeAuth, is_expired_with_buffer};
use super::oidc::{OidcRefreshResult, chatgpt_refresh, oidc_token_exchange};

/// Pre-refresh margin: treat a token as due for refresh this long before it
/// actually expires. Matches `auth_provider.rs`'s
/// `PROVIDER_TOKEN_EXPIRY_SKEW_SECS`.
const OAUTH_EXPIRY_SKEW_SECS: i64 = 60;

fn expiry_skew() -> chrono::Duration {
    chrono::Duration::seconds(OAUTH_EXPIRY_SKEW_SECS)
}

/// A model's reference to its provider's OAuth credential, attached by
/// `resolve_model_list` to every model with a `model_family`. Unlike
/// [`super::AuthProviderRef`] there is no config-layer attachment step: the
/// provider id plus the compiled-in catalog is enough to reconstitute
/// behavior, so a ref revived from a persisted session (`#[derive(Deserialize)]`)
/// is immediately usable with no `attach_*` call.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(crate) struct ProviderOAuthRef {
    provider_id: String,
}

/// Outcome of a pre-turn or 401-triggered refresh attempt.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a rotated token must be written to chat-state, or the wire keeps the stale key"]
pub(crate) enum ProviderOAuthOutcome {
    /// `current_key` is already the fresh stored token; nothing to write.
    Unchanged,
    /// A token that should replace `current_key` on the wire.
    Rotated(String),
    /// Nothing stored for this provider, or the stored credential isn't
    /// OAuth-mode (a plain API key belongs to the older tier instead).
    Unusable,
    /// A transient failure (network, IdP outage). The stored token is left
    /// as-is; a caller with an unexpired one may still use it.
    RefreshFailed,
    /// The IdP terminally rejected the refresh token (revoked, `invalid_grant`,
    /// ...). The stored credential is left on disk (so the provider manager
    /// can still show it and offer to sign in again) but will not resolve
    /// until the user does.
    ReauthRequired,
}

impl ProviderOAuthOutcome {
    pub(crate) fn rotated(self) -> Option<String> {
        match self {
            Self::Rotated(token) => Some(token),
            Self::Unchanged | Self::Unusable | Self::RefreshFailed | Self::ReauthRequired => None,
        }
    }
}

/// Per-provider single-flight lock so concurrent turns/sessions sharing one
/// provider credential refresh it once, not once per caller. Holds no
/// credential state itself -- disk is the source of truth -- only
/// coordinates who is allowed to call the IdP right now.
type FlightLock = std::sync::Arc<tokio::sync::Mutex<()>>;

static OAUTH_FLIGHT_LOCKS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, FlightLock>>,
> = std::sync::OnceLock::new();

fn flight_lock(provider_id: &str) -> FlightLock {
    let map = OAUTH_FLIGHT_LOCKS.get_or_init(Default::default);
    let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(provider_id.to_owned()).or_default().clone()
}

impl ProviderOAuthRef {
    pub(crate) fn new(provider_id: String) -> Self {
        Self { provider_id }
    }

    /// The stored credential for this provider, only if it's OAuth-mode.
    fn stored(&self, bcode_home: &Path) -> Option<BcodeAuth> {
        let auth = accounts::read_provider_auth(bcode_home, &self.provider_id)?;
        (auth.auth_mode == AuthMode::Oidc).then_some(auth)
    }

    /// Cache-only read for sync resolution: one disk read of `auth.json`,
    /// the same cost `resolve_credentials`'s plain provider-key tier already
    /// pays every call. Never refreshes over the network; that happens
    /// pre-turn via [`Self::ensure_fresh_token`]. `None` for no credential, a
    /// non-OAuth credential, or one due for refresh (skew-checked).
    pub(crate) fn cached_token(&self, bcode_home: &Path) -> Option<String> {
        let auth = self.stored(bcode_home)?;
        if is_expired_with_buffer(&auth, expiry_skew()) {
            return None;
        }
        Some(auth.key)
    }

    /// The token that should replace `current_key` on the wire: serves the
    /// stored token when fresh, refreshes it (single-flight per provider)
    /// when due.
    pub(crate) async fn ensure_fresh_token(
        &self,
        bcode_home: &Path,
        current_key: Option<&str>,
    ) -> ProviderOAuthOutcome {
        let Some(auth) = self.stored(bcode_home) else {
            return ProviderOAuthOutcome::Unusable;
        };
        if !is_expired_with_buffer(&auth, expiry_skew()) {
            return if current_key == Some(auth.key.as_str()) {
                ProviderOAuthOutcome::Unchanged
            } else {
                ProviderOAuthOutcome::Rotated(auth.key)
            };
        }
        let lock = flight_lock(&self.provider_id);
        let _guard = lock.lock().await;
        // Re-read: a concurrent waiter may have refreshed while this task queued for the lock.
        let Some(auth) = self.stored(bcode_home) else {
            return ProviderOAuthOutcome::Unusable;
        };
        if !is_expired_with_buffer(&auth, expiry_skew()) {
            return ProviderOAuthOutcome::Rotated(auth.key);
        }
        self.refresh_and_persist(bcode_home, auth).await
    }

    /// The replacement for a server-rejected `rejected_key`. A fresher stored
    /// token (refreshed by a sibling session sharing this provider) is
    /// adopted without a network call; otherwise a refresh is forced
    /// regardless of the skew window.
    pub(crate) async fn recover_rejected_token(
        &self,
        bcode_home: &Path,
        rejected_key: &str,
    ) -> Option<String> {
        let auth = self.stored(bcode_home)?;
        if auth.key != rejected_key {
            return Some(auth.key);
        }
        let lock = flight_lock(&self.provider_id);
        let _guard = lock.lock().await;
        let auth = self.stored(bcode_home)?;
        if auth.key != rejected_key {
            return Some(auth.key);
        }
        self.refresh_and_persist(bcode_home, auth).await.rotated()
    }

    /// The provider's ChatGPT-profile OAuth app, when the catalog says this
    /// provider signs in that way. `None` for plain-key providers and for any
    /// generic OIDC app.
    fn chatgpt_auth(&self) -> Option<&'static bcode_models::ProviderAuth> {
        let info = bcode_models::provider(&self.provider_id)?;
        match info.auth.as_ref() {
            Some(a) if a.profile == bcode_models::ProviderOAuthProfile::Chatgpt => Some(a),
            _ => None,
        }
    }

    async fn refresh_and_persist(
        &self,
        bcode_home: &Path,
        auth: BcodeAuth,
    ) -> ProviderOAuthOutcome {
        let result = match self.chatgpt_auth() {
            Some(provider_auth) => chatgpt_refresh(&auth, provider_auth).await,
            None => oidc_token_exchange(&auth).await,
        };
        match result {
            OidcRefreshResult::Success(new_auth) => {
                let key = new_auth.key.clone();
                if let Err(e) =
                    accounts::store_provider_oauth(bcode_home, &self.provider_id, *new_auth)
                {
                    tracing::warn!(
                        provider = %self.provider_id,
                        error = %e,
                        "provider OAuth: refreshed token failed to persist"
                    );
                }
                ProviderOAuthOutcome::Rotated(key)
            }
            OidcRefreshResult::TerminalError { reason } => {
                tracing::warn!(
                    provider = %self.provider_id,
                    ?reason,
                    "provider OAuth: refresh terminally rejected; sign-in required again"
                );
                ProviderOAuthOutcome::ReauthRequired
            }
            OidcRefreshResult::Failed {
                network_unreachable,
            } => {
                tracing::warn!(
                    provider = %self.provider_id,
                    network_unreachable,
                    "provider OAuth: refresh failed transiently"
                );
                ProviderOAuthOutcome::RefreshFailed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oauth_auth(key: &str, refresh_token: &str, expires_in_secs: i64) -> BcodeAuth {
        BcodeAuth {
            key: key.to_owned(),
            auth_mode: AuthMode::Oidc,
            refresh_token: Some(refresh_token.to_owned()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::seconds(expires_in_secs)),
            oidc_issuer: Some("https://issuer.example.invalid".to_owned()),
            oidc_client_id: Some("client-1".to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn cached_token_reads_a_fresh_stored_oauth_credential() {
        let dir = tempfile::tempdir().expect("tempdir");
        accounts::store_provider_oauth(dir.path(), "acme", oauth_auth("tok-1", "rt-1", 3600))
            .expect("store");
        let token = ProviderOAuthRef::new("acme".to_owned()).cached_token(dir.path());
        assert_eq!(token.as_deref(), Some("tok-1"));
    }

    #[test]
    fn a_plain_api_key_provider_credential_is_not_oauth_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        accounts::store_provider_key(dir.path(), "acme", "sk-plain").expect("store");
        assert_eq!(
            ProviderOAuthRef::new("acme".to_owned()).cached_token(dir.path()),
            None,
            "a plain API key must not be served through the OAuth-only path"
        );
    }

    #[test]
    fn nothing_stored_is_unusable_not_a_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            ProviderOAuthRef::new("acme".to_owned()).cached_token(dir.path()),
            None
        );
    }

    #[test]
    fn near_expiry_within_skew_is_not_served_from_cache() {
        let dir = tempfile::tempdir().expect("tempdir");
        accounts::store_provider_oauth(dir.path(), "acme", oauth_auth("tok-1", "rt-1", 30))
            .expect("store");
        assert_eq!(
            ProviderOAuthRef::new("acme".to_owned()).cached_token(dir.path()),
            None,
            "a token expiring in 30s is inside the 60s skew window"
        );
    }

    #[tokio::test]
    async fn ensure_fresh_token_refreshes_a_stale_credential_against_the_mock_idp() {
        crate::auth::oidc::test_helpers::ensure_crypto_provider();
        let (issuer, idp) = crate::auth::oidc::test_helpers::start_mock_idp().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let mut stale = oauth_auth("stale-token", "mock-refresh-token", -10);
        stale.oidc_issuer = Some(issuer.clone());
        stale.oidc_client_id = Some(crate::auth::oidc::test_helpers::TEST_CLIENT_ID.to_owned());
        accounts::store_provider_oauth(dir.path(), "acme", stale).expect("store");

        let provider = ProviderOAuthRef::new("acme".to_owned());
        let outcome = provider
            .ensure_fresh_token(dir.path(), Some("stale-token"))
            .await;
        let ProviderOAuthOutcome::Rotated(new_key) = outcome else {
            panic!("expected Rotated, got {outcome:?}");
        };
        assert_eq!(new_key, "mock-access-token");

        let persisted = accounts::read_provider_auth(dir.path(), "acme").expect("still stored");
        assert_eq!(persisted.key, "mock-access-token");
        assert_eq!(persisted.auth_mode, AuthMode::Oidc);
        assert_eq!(
            provider.cached_token(dir.path()).as_deref(),
            Some("mock-access-token"),
            "the refreshed token must be immediately servable from disk"
        );
        idp.abort();
    }

    #[tokio::test]
    async fn ensure_fresh_token_serves_the_cached_token_unchanged_when_not_due() {
        let dir = tempfile::tempdir().expect("tempdir");
        accounts::store_provider_oauth(dir.path(), "acme", oauth_auth("tok-1", "rt-1", 3600))
            .expect("store");
        let provider = ProviderOAuthRef::new("acme".to_owned());
        let outcome = provider.ensure_fresh_token(dir.path(), Some("tok-1")).await;
        assert_eq!(outcome, ProviderOAuthOutcome::Unchanged);
    }

    #[tokio::test]
    async fn recover_rejected_token_forces_a_refresh_against_the_mock_idp() {
        crate::auth::oidc::test_helpers::ensure_crypto_provider();
        let (issuer, idp) = crate::auth::oidc::test_helpers::start_mock_idp().await;
        let dir = tempfile::tempdir().expect("tempdir");
        // Not yet expired by the clock, but the server rejected it -- 401 recovery must force a refresh anyway.
        let mut rejected = oauth_auth("rejected-token", "mock-refresh-token", 3600);
        rejected.oidc_issuer = Some(issuer.clone());
        rejected.oidc_client_id = Some(crate::auth::oidc::test_helpers::TEST_CLIENT_ID.to_owned());
        accounts::store_provider_oauth(dir.path(), "acme", rejected).expect("store");

        let provider = ProviderOAuthRef::new("acme".to_owned());
        let recovered = provider
            .recover_rejected_token(dir.path(), "rejected-token")
            .await;
        assert_eq!(recovered.as_deref(), Some("mock-access-token"));
        idp.abort();
    }

    #[tokio::test]
    async fn recover_rejected_token_is_none_for_an_unusable_provider() {
        let dir = tempfile::tempdir().expect("tempdir");
        let provider = ProviderOAuthRef::new("acme".to_owned());
        assert_eq!(provider.recover_rejected_token(dir.path(), "x").await, None);
    }
}
