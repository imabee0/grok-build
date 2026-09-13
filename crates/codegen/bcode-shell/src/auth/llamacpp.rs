//! llama.cpp (llama-server) discovery: live model id and the running context
//! window, plus a compact-early policy for small `n_ctx` so the harness still
//! fits.
//!
//! Detection, in order of accuracy:
//! 1. `GET /props` → `default_generation_settings.n_ctx` (the slot size the
//!    server actually allocated — not the GGUF training window).
//! 2. `GET /v1/models` → `data[0].id` and optional `meta.n_ctx`.
//! 3. Catalog fallback (8192) when the server is down or the bodies are
//!    unreadable.
//!
//! Never use `meta.n_ctx_train`: a 128k-trained GGUF commonly runs at 4k–8k.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// Sentinel stored when llama-server has no `--api-key`.
pub const OPTIONAL_KEY: &str = "none";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub n_ctx: u64,
    pub model_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowPolicy {
    pub context_window: u64,
    pub auto_compact_threshold_percent: u8,
    pub max_completion_tokens: u32,
    pub use_concise: bool,
}

/// Compact earlier and cap output so input + generation stay inside `n_ctx`.
/// Does not strip tools: `use_concise` only swaps the shorter tool-description
/// pack. Thresholds stay at or below the catalog 85% default.
pub fn window_policy(n_ctx: u64) -> WindowPolicy {
    let n_ctx = n_ctx.max(1024);
    let auto_compact_threshold_percent = match n_ctx {
        0..=4096 => 55,
        4097..=8192 => 60,
        8193..=16384 => 70,
        16385..=32768 => 75,
        _ => 85,
    };
    let max_completion_tokens = (n_ctx / 8).clamp(256, 4096) as u32;
    WindowPolicy {
        context_window: n_ctx,
        auto_compact_threshold_percent,
        max_completion_tokens,
        use_concise: n_ctx < 16_384,
    }
}

/// OpenAI-compat `/v1` base → llama-server origin (`/props` is not under `/v1`).
pub fn native_origin(openai_base: &str) -> String {
    let trimmed = openai_base.trim().trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_string()
}

/// Cached blocking probe. Safe to call from sync sampling-config construction:
/// a hit is a mutex lookup; a miss pays one 2s HTTP round trip on a dedicated
/// thread so it never borrows the caller's runtime.
pub fn cached_probe(base_url: &str, api_key: Option<&str>) -> Option<Probe> {
    let key = base_url.trim().trim_end_matches('/').to_string();
    if key.is_empty() {
        return None;
    }
    if let Some(hit) = cache_lock().get(&key).cloned() {
        return Some(hit);
    }
    let probe_key = key.clone();
    let auth = api_key
        .filter(|k| !k.is_empty() && *k != OPTIONAL_KEY)
        .map(str::to_owned);
    let result = std::thread::spawn(move || probe_blocking(&probe_key, auth.as_deref()))
        .join()
        .ok()
        .flatten();
    if let Some(ref probe) = result {
        cache_lock().insert(key, probe.clone());
    }
    result
}

fn cache_lock() -> std::sync::MutexGuard<'static, HashMap<String, Probe>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Probe>>> = OnceLock::new();
    CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn probe_blocking(base_url: &str, api_key: Option<&str>) -> Option<Probe> {
    let client = crate::http::shared_startup_blocking_client();
    let origin = native_origin(base_url);
    let models_url = format!("{}/models", base_url.trim_end_matches('/'));
    let props_url = format!("{origin}/props");

    let models = get_json(&client, &models_url, api_key);
    let props = get_json(&client, &props_url, api_key);

    let model_id = models.as_ref().and_then(parse_models_id);
    let n_ctx = props
        .as_ref()
        .and_then(parse_props_n_ctx)
        .or_else(|| models.as_ref().and_then(parse_models_n_ctx))?;
    if n_ctx == 0 {
        return None;
    }
    Some(Probe { n_ctx, model_id })
}

fn get_json(
    client: &reqwest::blocking::Client,
    url: &str,
    api_key: Option<&str>,
) -> Option<serde_json::Value> {
    let mut req = client.get(url).timeout(PROBE_TIMEOUT);
    if let Some(key) = api_key {
        req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"));
    }
    let resp = req.send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().ok()
}

#[derive(Deserialize)]
struct ModelsList {
    #[serde(default)]
    data: Vec<ModelRow>,
}

#[derive(Deserialize)]
struct ModelRow {
    id: Option<String>,
    #[serde(default)]
    meta: Option<ModelMeta>,
}

#[derive(Deserialize)]
struct ModelMeta {
    #[serde(default)]
    n_ctx: Option<u64>,
}

#[derive(Deserialize)]
struct Props {
    #[serde(default)]
    default_generation_settings: Option<GenerationSettings>,
}

#[derive(Deserialize)]
struct GenerationSettings {
    #[serde(default)]
    n_ctx: Option<u64>,
}

pub fn parse_props_n_ctx(value: &serde_json::Value) -> Option<u64> {
    serde_json::from_value::<Props>(value.clone())
        .ok()?
        .default_generation_settings?
        .n_ctx
        .filter(|n| *n > 0)
}

pub fn parse_models_id(value: &serde_json::Value) -> Option<String> {
    serde_json::from_value::<ModelsList>(value.clone())
        .ok()?
        .data
        .into_iter()
        .find_map(|row| row.id.filter(|id| !id.is_empty()))
}

pub fn parse_models_n_ctx(value: &serde_json::Value) -> Option<u64> {
    serde_json::from_value::<ModelsList>(value.clone())
        .ok()?
        .data
        .iter()
        .find_map(|row| row.meta.as_ref().and_then(|m| m.n_ctx))
        .filter(|n| *n > 0)
}

/// Apply a successful probe onto a sampler config, unless the user already
/// pinned `context_window` in `[model.<id>]`.
pub fn apply_probe(
    context_window: &mut u64,
    max_completion_tokens: &mut Option<u32>,
    model_name: &mut String,
    placeholder_model: &str,
    user_pinned_window: bool,
    probe: &Probe,
) {
    let policy = window_policy(probe.n_ctx);
    if !user_pinned_window {
        *context_window = policy.context_window;
    }
    if max_completion_tokens.is_none() {
        *max_completion_tokens = Some(policy.max_completion_tokens);
    }
    if let Some(id) = probe.model_id.as_deref()
        && (model_name == placeholder_model || model_name.is_empty())
    {
        *model_name = id.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_origin_strips_v1() {
        assert_eq!(
            native_origin("http://127.0.0.1:8080/v1"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            native_origin("http://127.0.0.1:8080/v1/"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            native_origin("http://127.0.0.1:8080"),
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn props_n_ctx_is_the_running_slot_size() {
        let v = serde_json::json!({
            "default_generation_settings": { "n_ctx": 4096 }
        });
        assert_eq!(parse_props_n_ctx(&v), Some(4096));
    }

    #[test]
    fn models_list_yields_id_and_optional_n_ctx_not_train() {
        let v = serde_json::json!({
            "data": [{
                "id": "qwen-coder",
                "owned_by": "llamacpp",
                "meta": { "n_ctx_train": 262144, "n_ctx": 8192 }
            }]
        });
        assert_eq!(parse_models_id(&v).as_deref(), Some("qwen-coder"));
        assert_eq!(parse_models_n_ctx(&v), Some(8192));
    }

    #[test]
    fn n_ctx_train_alone_is_ignored() {
        let v = serde_json::json!({
            "data": [{ "id": "m", "meta": { "n_ctx_train": 131072 } }]
        });
        assert_eq!(parse_models_n_ctx(&v), None);
    }

    #[test]
    fn window_policy_compacts_earlier_on_small_ctx() {
        let tiny = window_policy(2048);
        assert_eq!(tiny.context_window, 2048);
        assert_eq!(tiny.auto_compact_threshold_percent, 55);
        assert_eq!(tiny.max_completion_tokens, 256);
        assert!(tiny.use_concise);
        // 55% of 2048 + 256 output = 1382, well under the window.
        assert!(
            tiny.context_window * u64::from(tiny.auto_compact_threshold_percent) / 100
                + u64::from(tiny.max_completion_tokens)
                < tiny.context_window
        );

        let mid = window_policy(8192);
        assert_eq!(mid.auto_compact_threshold_percent, 60);
        assert_eq!(mid.max_completion_tokens, 1024);
        assert!(mid.use_concise);

        let big = window_policy(65536);
        assert_eq!(big.auto_compact_threshold_percent, 85);
        assert!(!big.use_concise);
        assert_eq!(big.max_completion_tokens, 4096);
    }

    #[test]
    fn apply_probe_fills_placeholder_but_respects_a_pinned_window() {
        let probe = Probe {
            n_ctx: 4096,
            model_id: Some("qwen-coder".into()),
        };
        let mut cw = 8192;
        let mut max_out = None;
        let mut name = "llamacpp".to_string();
        apply_probe(&mut cw, &mut max_out, &mut name, "llamacpp", false, &probe);
        assert_eq!(cw, 4096);
        assert_eq!(max_out, Some(512));
        assert_eq!(name, "qwen-coder");

        let mut cw = 16384;
        let mut max_out = Some(2000);
        let mut name = "llamacpp".to_string();
        apply_probe(&mut cw, &mut max_out, &mut name, "llamacpp", true, &probe);
        assert_eq!(cw, 16384, "user [model].context_window must win");
        assert_eq!(max_out, Some(2000), "user max_completion_tokens must win");
    }
}
