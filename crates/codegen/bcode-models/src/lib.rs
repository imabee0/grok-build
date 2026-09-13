//! Default model IDs loaded from `default_models.json` at runtime.
//! Edit that JSON file to change them.
//!
//! At runtime each model is resolved from the first of these that is set: CLI flag, ENV var, config.toml, remote settings, these defaults.

use std::sync::LazyLock;

/// The raw JSON, embedded at compile time.
/// It is `pub` because `bcode_shell::models` re-exports it and `agent::config` reads it.
pub const DEFAULT_MODELS_JSON: &str = include_str!("../default_models.json");

/// Per-model rates, next to the catalog because they are the same kind of
/// thing: provider data keyed by wire model id. Parsed by `bcode-pricing`.
pub const PRICING_TOML: &str = include_str!("../pricing.toml");

#[derive(serde::Deserialize)]
struct DefaultModels {
    default: String,
    /// Falls back to `default` if not specified in JSON.
    web_search: Option<String>,
    /// Falls back to `default` if not specified in JSON.
    image_description: Option<String>,
    /// Falls back to `default` if not specified in JSON.
    session_summary: Option<String>,
    models: Vec<DefaultModelEntry>,
    #[serde(default)]
    providers: Vec<ProviderInfo>,
}

#[derive(serde::Deserialize)]
struct DefaultModelEntry {
    model: String,
    model_family: Option<String>,
}

/// One `providers` row: the registry `bcode login` and the provider-scoped
/// credential tier read. A model's provider is its `model_family`; a
/// credential stored for a provider id works for every model that shares it.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub base_url: String,
    /// The provider's own conventional env var name for its API key.
    pub env_key: String,
    /// Catalog id of the model `bcode login` offers to set as the default
    /// after a successful sign-in to this provider.
    pub default_model: String,
    /// Where a user without a key yet can go create one.
    pub api_key_url: String,
    /// OAuth2/OIDC subscription sign-in for this provider, if it offers one.
    /// Absent for every provider in this file today -- none of them expose a
    /// public OAuth app for API access, so there is no truthful data to put
    /// here. The mechanism this drives is proven against the in-tree mock
    /// IdP; a real entry only belongs here once a provider actually
    /// publishes real issuer/client-id values.
    #[serde(default)]
    pub auth: Option<ProviderAuth>,
    /// How to discover the live model id and context window. `None` is a
    /// static catalog row; `Llamacpp` probes the local OpenAI-compat server.
    #[serde(default)]
    pub probe: ProviderProbe,
}

/// Runtime discovery a catalog provider can run against its own server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProbe {
    #[default]
    None,
    /// llama-server: `GET /health`, `GET /v1/models`, `GET /props` (`n_ctx`).
    Llamacpp,
}

/// A provider's own OAuth2/OIDC app for subscription sign-in, independent of
/// its API-key tier. Every field comes from the provider's own published
/// values -- never invented -- which is why every provider in this file
/// leaves this unset.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ProviderAuth {
    pub issuer: String,
    pub client_id: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Loopback ports to try, in order, for the OAuth redirect_uri. Real
    /// vendors pre-register exact ports rather than accepting any; this list
    /// must be non-empty whenever `auth` is set.
    pub redirect_ports: Vec<u16>,
    /// Extra headers this provider's inference API requires alongside the
    /// bearer token (e.g. an account-id header), sent verbatim.
    #[serde(default)]
    pub inference_headers: std::collections::BTreeMap<String, String>,
    /// Which protocol the sign-in and refresh run. `Oidc` (the default) is
    /// discovery-driven; `Chatgpt` uses hardcoded endpoint paths plus a
    /// post-login token-exchange that mints the API-usable key.
    #[serde(default)]
    pub profile: ProviderOAuthProfile,
    /// Authorization endpoint path under `issuer` (e.g. "/oauth/authorize").
    /// Unused by the discovery-driven `Oidc` profile.
    #[serde(default)]
    pub authorize_path: Option<String>,
    /// Token endpoint path under `issuer` (e.g. "/oauth/token"). Unused by the
    /// discovery-driven `Oidc` profile.
    #[serde(default)]
    pub token_path: Option<String>,
    /// Loopback host used in the redirect_uri. Defaults to "127.0.0.1"; the
    /// ChatGPT app pre-registers "localhost" instead.
    #[serde(default)]
    pub redirect_host: Option<String>,
    /// Callback path on the loopback host. Defaults to "/callback"; the
    /// ChatGPT app pre-registers "/auth/callback".
    #[serde(default)]
    pub redirect_path: Option<String>,
    /// Extra query params appended to the authorize URL verbatim (e.g.
    /// `id_token_add_organizations=true`).
    #[serde(default)]
    pub authorize_extra: std::collections::BTreeMap<String, String>,
    /// Token-exchange `requested_token` value (e.g. "openai-api-key"). When
    /// set, the login's id_token is exchanged for this token type, which
    /// becomes the stored credential key.
    #[serde(default)]
    pub requested_token: Option<String>,
    /// Refresh uses a JSON body (`true`, ChatGPT) instead of form encoding
    /// (`false`, standard OAuth2 refresh_token grant).
    #[serde(default)]
    pub json_refresh: bool,
}

/// The OAuth protocol a provider's sign-in and refresh run.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderOAuthProfile {
    /// Standard OIDC: discovery-driven endpoints, `refresh_token` grant via
    /// form encoding, access token is the credential key.
    #[default]
    Oidc,
    /// ChatGPT: hardcoded `/oauth/authorize` + `/oauth/token`, a post-login
    /// token-exchange that mints the API key, and JSON refresh.
    Chatgpt,
}

static DEFAULTS: LazyLock<DefaultModels> = LazyLock::new(|| {
    let defaults: DefaultModels = serde_json::from_str(DEFAULT_MODELS_JSON)
        .expect("default_models.json: invalid JSON or missing 'default' field");

    // Baked-in JSON: a mismatch here is a developer error
    let model_ids: Vec<&str> = defaults.models.iter().map(|m| m.model.as_str()).collect();
    assert!(
        model_ids.contains(&defaults.default.as_str()),
        "default_models.json: 'default' is '{}' but 'models' array only has {model_ids:?}",
        defaults.default,
    );
    let provider_ids: Vec<&str> = defaults.providers.iter().map(|p| p.id.as_str()).collect();
    for model in &defaults.models {
        if let Some(family) = model.model_family.as_deref() {
            assert!(
                provider_ids.contains(&family),
                "default_models.json: model '{}' has model_family '{family}',                  which has no matching entry in 'providers' ({provider_ids:?})",
                model.model,
            );
        }
    }
    for provider in &defaults.providers {
        assert!(
            model_ids.contains(&provider.default_model.as_str()),
            "default_models.json: provider '{}' has default_model '{}',              which is not in 'models' ({model_ids:?})",
            provider.id,
            provider.default_model,
        );
        if let Some(auth) = &provider.auth {
            assert!(
                !auth.issuer.trim().is_empty() && !auth.client_id.trim().is_empty(),
                "default_models.json: provider '{}' has an 'auth' entry with an empty issuer or client_id",
                provider.id,
            );
            assert!(
                !auth.redirect_ports.is_empty(),
                "default_models.json: provider '{}' has an 'auth' entry with no redirect_ports",
                provider.id,
            );
        }
    }

    defaults
});

/// The provider registry: one row per provider, in catalog order.
///
/// A model's provider is its `model_family`; look a model up by matching
/// [`ModelInfo::model_family`](../bcode_shell/agent/config/struct.ModelInfo.html)
/// (or the raw JSON field of the same name) against [`ProviderInfo::id`].
pub fn providers() -> &'static [ProviderInfo] {
    &DEFAULTS.providers
}

/// The provider registered under `id`, if any.
pub fn provider(id: &str) -> Option<&'static ProviderInfo> {
    DEFAULTS.providers.iter().find(|p| p.id == id)
}

/// Primary model for coding tasks and general fallback.
pub fn default_model() -> &'static str {
    &DEFAULTS.default
}

/// Model for web search tool synthesis. Falls back to default model.
pub fn default_web_search_model() -> &'static str {
    DEFAULTS.web_search.as_deref().unwrap_or(&DEFAULTS.default)
}

/// Model for image describe. Falls back to default model.
pub fn default_image_description_model() -> &'static str {
    DEFAULTS
        .image_description
        .as_deref()
        .unwrap_or(&DEFAULTS.default)
}

/// Model for session title generation. Falls back to default model.
pub fn default_session_summary_model() -> &'static str {
    DEFAULTS
        .session_summary
        .as_deref()
        .unwrap_or(&DEFAULTS.default)
}
