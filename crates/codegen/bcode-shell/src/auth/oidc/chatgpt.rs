//! ChatGPT provider OAuth: the protocol differences from generic OIDC.
//!
//! The ChatGPT app (issuer `https://auth.openai.com`) predates the generic
//! discovery-driven flow in `protocol.rs`/`login.rs`. It differs in four ways,
//! each pinned here against the provider's own open-source CLI:
//!
//! 1. Endpoints are hardcoded paths under the issuer (`/oauth/authorize`,
//!    `/oauth/token`), not read from `.well-known/openid-configuration`.
//! 2. The loopback redirect uses `localhost` and `/auth/callback` on one of
//!    two pre-registered ports.
//! 3. The `authorization_code` access token is *not* the API key: the login's
//!    `id_token` is exchanged (RFC 8693 token-exchange) for the token type the
//!    provider's inference API actually accepts, and *that* is the key.
//! 4. Refresh posts a JSON `refresh_token` grant (not form-encoded), and the
//!    grant response carries no `id_token`, so the API key is re-minted only
//!    when the IdP happens to return one.
//!
//! Only the mechanics live here; every provider value (issuer, client id,
//! ports, scope, requested token type) is catalog data in `bcode-models`.

use std::time::Duration as StdDuration;

use super::super::BcodeAuth;
use super::protocol::{OidcError, OidcUserInfo, TokenResponse, build_bcode_auth};
use super::refresh::{OidcRefreshResult, classify_terminal, is_network_unreachable};
use crate::auth::error::RefreshTokenFailedReason;
use crate::auth::jwt::ensure_crypto_provider;
use bcode_models::ProviderAuth;
use indexmap::IndexMap;

/// RFC 8693 token-exchange grant type.
const TOKEN_EXCHANGE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
/// RFC 8693 `subject_token_type` for an OIDC id_token.
const ID_TOKEN_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:id_token";

fn issuer_root(provider: &ProviderAuth) -> &str {
    provider.issuer.trim_end_matches('/')
}

pub(super) fn authorize_endpoint(provider: &ProviderAuth) -> String {
    format!(
        "{}{}",
        issuer_root(provider),
        provider
            .authorize_path
            .as_deref()
            .unwrap_or("/oauth/authorize")
    )
}

pub(super) fn token_endpoint(provider: &ProviderAuth) -> String {
    format!(
        "{}{}",
        issuer_root(provider),
        provider.token_path.as_deref().unwrap_or("/oauth/token")
    )
}

/// The loopback redirect host (`localhost` for ChatGPT, else `127.0.0.1`).
pub(super) fn redirect_host(provider: &ProviderAuth) -> String {
    provider
        .redirect_host
        .clone()
        .unwrap_or_else(|| "127.0.0.1".to_owned())
}

/// The callback path on the loopback host (`/auth/callback` for ChatGPT).
pub(super) fn redirect_path(provider: &ProviderAuth) -> String {
    provider
        .redirect_path
        .clone()
        .unwrap_or_else(|| "/callback".to_owned())
}

/// Build the ChatGPT authorize URL: hardcoded endpoint, the provider's scope,
/// PKCE S256, and the provider's extra query params.
pub(super) fn build_authorize_url(
    provider: &ProviderAuth,
    redirect_uri: &str,
    pkce: &super::protocol::Pkce,
    state: &str,
) -> String {
    let mut query = vec![
        ("response_type".to_string(), "code".to_string()),
        ("client_id".to_string(), provider.client_id.clone()),
        ("redirect_uri".to_string(), redirect_uri.to_string()),
        ("scope".to_string(), provider.scopes.join(" ")),
        ("code_challenge".to_string(), pkce.code_challenge.clone()),
        ("code_challenge_method".to_string(), "S256".to_string()),
        ("state".to_string(), state.to_string()),
    ];
    for (key, value) in &provider.authorize_extra {
        query.push((key.clone(), value.clone()));
    }
    let qs = query
        .into_iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(&v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{qs}", authorize_endpoint(provider))
}

#[derive(Debug, serde::Deserialize)]
struct ApiKeyExchangeResp {
    access_token: String,
}

/// Exchange an id_token for the API-usable token via RFC 8693 token-exchange.
///
/// The provider's `requested_token` (e.g. `openai-api-key`) names the token
/// type its inference API accepts; the returned `access_token` is the key.
pub(super) async fn obtain_api_key(
    provider: &ProviderAuth,
    id_token: &str,
) -> anyhow::Result<String> {
    let endpoint = token_endpoint(provider);
    let requested_token = provider.requested_token.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "provider '{}' is chatgpt-profile but has no requested_token",
            provider.client_id
        )
    })?;
    tracing::debug!(token_endpoint = %endpoint, "ChatGPT: exchanging id_token for API key");
    let resp = crate::http::shared_client()
        .post(&endpoint)
        .form(&[
            ("grant_type", TOKEN_EXCHANGE_GRANT_TYPE),
            ("client_id", provider.client_id.as_str()),
            ("requested_token", requested_token),
            ("subject_token", id_token),
            ("subject_token_type", ID_TOKEN_TOKEN_TYPE),
        ])
        .timeout(StdDuration::from_secs(15))
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::Error::new(OidcError::TokenExchangeHttp {
            status,
            body,
        }));
    }
    Ok(resp.json::<ApiKeyExchangeResp>().await?.access_token)
}

/// ChatGPT Codex inference host. A ChatGPT OAuth access token is not a
/// platform `openai-api-key`; Codex talks here, not `api.openai.com`.
pub(crate) const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

#[derive(Debug, Default, serde::Deserialize)]
struct ChatgptAuthClaim {
    #[serde(default)]
    organization_id: Option<String>,
    #[serde(default)]
    chatgpt_account_id: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct ChatgptJwtClaims {
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: Option<ChatgptAuthClaim>,
}

/// Decode ChatGPT IdP claims from an access or id token. Signature is not
/// checked: the token came from the token endpoint we just called, or from
/// `auth.json` we wrote after that call.
fn chatgpt_jwt_claims(token: &str) -> ChatgptJwtClaims {
    ensure_crypto_provider();
    jsonwebtoken::dangerous::insecure_decode::<ChatgptJwtClaims>(token)
        .map(|d| d.claims)
        .unwrap_or_default()
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

pub(crate) fn chatgpt_account_id_from_token(token: &str) -> Option<String> {
    nonempty(
        chatgpt_jwt_claims(token)
            .auth
            .and_then(|a| a.chatgpt_account_id),
    )
}

pub(crate) fn chatgpt_organization_id_from_token(token: &str) -> Option<String> {
    nonempty(
        chatgpt_jwt_claims(token)
            .auth
            .and_then(|a| a.organization_id),
    )
}

/// Whether `auth.key` is a ChatGPT subscription JWT rather than a minted
/// platform API key (`sk-…`).
fn is_chatgpt_subscription_key(key: &str) -> bool {
    let key = key.trim();
    key.starts_with("eyJ") && !key.starts_with("sk-")
}

/// Base URL and extra headers for a stored OpenAI provider credential.
///
/// A minted `openai-api-key` stays on the catalog URL. A ChatGPT access
/// token is sent to the Codex ChatGPT backend with `ChatGPT-Account-ID`.
pub(crate) fn chatgpt_inference_route(
    auth: &BcodeAuth,
    catalog_base_url: &str,
) -> (String, IndexMap<String, String>) {
    let openai_issuer = auth
        .oidc_issuer
        .as_deref()
        .is_some_and(|iss| iss.trim_end_matches('/') == "https://auth.openai.com");
    if !openai_issuer || !is_chatgpt_subscription_key(&auth.key) {
        return (catalog_base_url.to_owned(), IndexMap::new());
    }
    let account_id = auth
        .chatgpt_account_id
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| chatgpt_account_id_from_token(&auth.key));
    let mut headers = IndexMap::new();
    if let Some(id) = account_id {
        headers.insert("ChatGPT-Account-ID".into(), id);
    }
    headers.insert("originator".into(), "bcode".into());
    headers.insert("OpenAI-Beta".into(), "responses=v1".into());
    (CHATGPT_CODEX_BASE_URL.to_owned(), headers)
}

#[derive(Debug, serde::Deserialize)]
struct RefreshResponse {
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
}

/// Extract the OAuth2 error code from a token-endpoint error body, accepting
/// both the flat `{"error":"invalid_grant"}` shape and the nested
/// `{"error":{"code":"refresh_token_expired"}}` shape the ChatGPT endpoint uses.
fn extract_error_code(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if let Some(code) = value.get("error") {
        if let Some(s) = code.as_str() {
            return Some(s.to_string());
        }
        if let Some(s) = code.get("code").and_then(|c| c.as_str()) {
            return Some(s.to_string());
        }
    }
    value
        .get("code")
        .and_then(|c| c.as_str())
        .map(str::to_string)
}

/// Terminal ChatGPT refresh errors map onto bcode's single `RefreshTokenRejected`
/// reason, which already means "the refresh token is dead, re-authenticate".
fn classify_chatgpt_terminal(error_code: &str) -> Option<RefreshTokenFailedReason> {
    match error_code {
        "invalid_grant"
        | "refresh_token_expired"
        | "refresh_token_reused"
        | "refresh_token_invalidated" => Some(RefreshTokenFailedReason::RefreshTokenRejected),
        other => classify_terminal(other),
    }
}

/// Refresh a ChatGPT credential: JSON `refresh_token` grant, then re-mint the
/// API key via token-exchange only when the grant returns a fresh `id_token`
/// (it usually does not -- the existing key is kept).
///
/// Pure data return; the caller (`ProviderOAuthRef`) persists the result.
pub(crate) async fn refresh(auth: &BcodeAuth, provider: &ProviderAuth) -> OidcRefreshResult {
    let Some(refresh_tok) = auth.refresh_token.as_ref() else {
        return OidcRefreshResult::Failed {
            network_unreachable: false,
        };
    };
    let Some(client_id) = auth.oidc_client_id.as_ref().or(Some(&provider.client_id)) else {
        return OidcRefreshResult::Failed {
            network_unreachable: false,
        };
    };
    let Some(issuer) = auth.oidc_issuer.as_ref() else {
        return OidcRefreshResult::Failed {
            network_unreachable: false,
        };
    };

    let endpoint = token_endpoint(provider);
    tracing::debug!(token_endpoint = %endpoint, client_id = %client_id, "ChatGPT: refreshing token");
    let resp = crate::http::shared_client()
        .post(&endpoint)
        .json(&serde_json::json!({
            "client_id": client_id,
            "grant_type": "refresh_token",
            "refresh_token": refresh_tok,
        }))
        .timeout(StdDuration::from_secs(15))
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            let network_unreachable = is_network_unreachable(&anyhow::Error::new(e));
            return OidcRefreshResult::Failed {
                network_unreachable,
            };
        }
    };

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        let error_code = extract_error_code(&body);
        if let Some(code) = error_code.as_deref()
            && let Some(reason) = classify_chatgpt_terminal(code)
        {
            tracing::warn!(
                http_status = status,
                oauth2_error = %code,
                client_id = %client_id,
                "ChatGPT: refresh terminally rejected"
            );
            return OidcRefreshResult::TerminalError { reason };
        }
        return OidcRefreshResult::Failed {
            network_unreachable: false,
        };
    }

    let tokens: RefreshResponse = match resp.json().await {
        Ok(t) => t,
        Err(e) => {
            return OidcRefreshResult::Failed {
                network_unreachable: is_network_unreachable(&anyhow::Error::new(e)),
            };
        }
    };

    // Re-mint the API key only when the grant carried a fresh id_token.
    let new_key = match tokens.id_token.as_deref() {
        Some(id_token) => match obtain_api_key(provider, id_token).await {
            Ok(key) => Some(key),
            Err(e) => {
                tracing::warn!(error = %e, "ChatGPT: refresh re-mint failed, keeping existing key");
                None
            }
        },
        None => None,
    };

    let user_info = OidcUserInfo {
        user_id: auth.user_id.clone(),
        email: auth.email.clone(),
        first_name: auth.first_name.clone(),
        last_name: auth.last_name.clone(),
        profile_image_asset_id: auth.profile_image_asset_id.clone(),
        principal_type: auth.principal_type.clone(),
        principal_id: auth.principal_id.clone(),
        team_id: auth.team_id.clone(),
        team_name: auth.team_name.clone(),
        team_role: auth.team_role.clone(),
        organization_id: auth.organization_id.clone(),
        organization_name: auth.organization_name.clone(),
        organization_role: auth.organization_role.clone(),
        user_blocked_reason: auth.user_blocked_reason.clone(),
        team_blocked_reasons: auth.team_blocked_reasons.clone(),
        coding_data_retention_opt_out: auth.coding_data_retention_opt_out,
    };
    let new_refresh_token = tokens.refresh_token.or_else(|| auth.refresh_token.clone());
    let mut new_auth = build_bcode_auth(
        TokenResponse {
            access_token: tokens.access_token.unwrap_or_else(|| auth.key.clone()),
            refresh_token: new_refresh_token,
            id_token: tokens.id_token,
            expires_in: None,
        },
        user_info,
        issuer,
        client_id,
    );
    new_auth.key = new_key.unwrap_or_else(|| auth.key.clone());
    OidcRefreshResult::Success(Box::new(new_auth))
}

#[cfg(test)]
mod tests {
    use super::super::super::{AuthMode, BcodeAuth};
    use super::super::protocol::generate_pkce;
    use super::*;

    fn provider(issuer: &str) -> ProviderAuth {
        ProviderAuth {
            issuer: issuer.to_string(),
            client_id: "test-client-id".to_string(),
            scopes: vec!["openid".into(), "profile".into(), "offline_access".into()],
            redirect_ports: vec![1455, 1457],
            inference_headers: Default::default(),
            profile: bcode_models::ProviderOAuthProfile::Chatgpt,
            authorize_path: Some("/oauth/authorize".into()),
            token_path: Some("/oauth/token".into()),
            redirect_host: Some("localhost".into()),
            redirect_path: Some("/auth/callback".into()),
            authorize_extra: [("id_token_add_organizations".to_string(), "true".to_string())]
                .into_iter()
                .collect(),
            requested_token: Some("test-api-key".into()),
            json_refresh: true,
        }
    }

    fn auth(issuer: &str, key: &str, refresh_token: &str) -> BcodeAuth {
        BcodeAuth {
            key: key.to_string(),
            auth_mode: AuthMode::Oidc,
            refresh_token: Some(refresh_token.to_string()),
            oidc_issuer: Some(issuer.to_string()),
            oidc_client_id: Some("test-client-id".to_string()),
            ..BcodeAuth::test_default()
        }
    }

    #[test]
    fn authorize_url_carries_scope_pkce_and_extra_params() {
        let provider = provider("https://auth.test.invalid");
        let pkce = generate_pkce();
        let url = build_authorize_url(
            &provider,
            "http://localhost:1455/auth/callback",
            &pkce,
            "state-1",
        );
        assert!(url.starts_with("https://auth.test.invalid/oauth/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=test-client-id"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
        assert!(url.contains("scope=openid%20profile%20offline_access"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("code_challenge="));
        assert!(url.contains("state=state-1"));
        assert!(url.contains("id_token_add_organizations=true"));
    }

    async fn start_mock_token_endpoint(
        refresh_id_token: Option<String>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let app = axum::Router::new().route(
            "/oauth/token",
            axum::routing::post(move |body: String| {
                let refresh_id_token = refresh_id_token.clone();
                async move {
                    if body.contains("token-exchange") {
                        axum::Json(serde_json::json!({ "access_token": "mock-api-key" }))
                    } else if body.contains("refresh_token") {
                        let mut resp = serde_json::json!({
                            "access_token": "mock-jwt",
                            "refresh_token": "mock-rt-2",
                        });
                        if let Some(idt) = refresh_id_token {
                            resp["id_token"] = serde_json::json!(idt);
                        }
                        axum::Json(resp)
                    } else {
                        axum::Json(serde_json::json!({
                            "access_token": "mock-access",
                            "id_token": "mock-id-token",
                            "refresh_token": "mock-rt-1",
                        }))
                    }
                }
            }),
        );
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (issuer, handle)
    }

    #[tokio::test]
    async fn obtain_api_key_exchanges_id_token() {
        let (issuer, handle) = start_mock_token_endpoint(None).await;
        let key = obtain_api_key(&provider(&issuer), "mock-id-token")
            .await
            .unwrap();
        assert_eq!(key, "mock-api-key");
        handle.abort();
    }

    #[tokio::test]
    async fn refresh_re_mints_key_when_id_token_returned() {
        let (issuer, handle) = start_mock_token_endpoint(Some("fresh-id-token".into())).await;
        let result = refresh(&auth(&issuer, "old-key", "mock-rt-1"), &provider(&issuer)).await;
        let OidcRefreshResult::Success(new_auth) = result else {
            panic!("expected Success, got {result:?}");
        };
        assert_eq!(new_auth.key, "mock-api-key");
        assert_eq!(new_auth.refresh_token.as_deref(), Some("mock-rt-2"));
        handle.abort();
    }

    #[tokio::test]
    async fn refresh_keeps_key_when_no_id_token_returned() {
        let (issuer, handle) = start_mock_token_endpoint(None).await;
        let result = refresh(&auth(&issuer, "old-key", "mock-rt-1"), &provider(&issuer)).await;
        let OidcRefreshResult::Success(new_auth) = result else {
            panic!("expected Success, got {result:?}");
        };
        assert_eq!(new_auth.key, "old-key");
        assert_eq!(new_auth.refresh_token.as_deref(), Some("mock-rt-2"));
        handle.abort();
    }

    #[tokio::test]
    async fn refresh_returns_terminal_error_on_invalid_grant() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let app = axum::Router::new().route(
            "/oauth/token",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({ "error": "invalid_grant" })),
                )
            }),
        );
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = refresh(&auth(&issuer, "old-key", "mock-rt-1"), &provider(&issuer)).await;
        assert!(
            matches!(result, OidcRefreshResult::TerminalError { .. }),
            "expected TerminalError, got {result:?}"
        );
        handle.abort();
    }

    #[test]
    fn extract_error_code_handles_flat_and_nested_shapes() {
        assert_eq!(
            extract_error_code(r#"{"error":"invalid_grant"}"#).as_deref(),
            Some("invalid_grant")
        );
        assert_eq!(
            extract_error_code(r#"{"error":{"code":"refresh_token_expired"}}"#).as_deref(),
            Some("refresh_token_expired")
        );
        assert_eq!(extract_error_code("not json"), None);
    }

    fn jwt_with_payload(payload_json: &str) -> String {
        use base64::Engine;
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = enc.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = enc.encode(payload_json);
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn jwt_claims_read_chatgpt_account_and_org() {
        let token = jwt_with_payload(
            r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct-1","organization_id":"org-9"}}"#,
        );
        assert_eq!(
            chatgpt_account_id_from_token(&token).as_deref(),
            Some("acct-1")
        );
        assert_eq!(
            chatgpt_organization_id_from_token(&token).as_deref(),
            Some("org-9")
        );
    }

    #[test]
    fn inference_route_uses_codex_backend_for_chatgpt_jwt() {
        let token =
            jwt_with_payload(r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct-1"}}"#);
        let auth = BcodeAuth {
            key: token,
            oidc_issuer: Some("https://auth.openai.com".into()),
            ..BcodeAuth::test_default()
        };
        let (url, headers) = chatgpt_inference_route(&auth, "https://api.openai.com/v1");
        assert_eq!(url, CHATGPT_CODEX_BASE_URL);
        assert_eq!(
            headers.get("ChatGPT-Account-ID").map(String::as_str),
            Some("acct-1")
        );
        assert_eq!(headers.get("originator").map(String::as_str), Some("bcode"));
        assert_eq!(
            headers.get("OpenAI-Beta").map(String::as_str),
            Some("responses=v1")
        );
    }

    #[test]
    fn inference_route_keeps_platform_url_for_minted_api_key() {
        let auth = BcodeAuth {
            key: "sk-live-not-a-jwt".into(),
            oidc_issuer: Some("https://auth.openai.com".into()),
            chatgpt_account_id: Some("acct-1".into()),
            ..BcodeAuth::test_default()
        };
        let (url, headers) = chatgpt_inference_route(&auth, "https://api.openai.com/v1");
        assert_eq!(url, "https://api.openai.com/v1");
        assert!(headers.is_empty());
    }
}
