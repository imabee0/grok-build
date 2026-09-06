//! The bcode backend: bcode OAuth2, enterprise OIDC, the operator's auth binary, or a devbox.

use std::sync::Arc;

use super::{AuthBackend, LoginRequest};
use crate::auth::refresh::{
    AuthSnapshot, DiagnosticUploader, ExternalBinaryRefresher, ExternalCommandRunner,
    OidcRefresher, TokenRefresher,
};
use crate::auth::{AuthManager, BcodeAuth, BcodeComConfig};

#[derive(Default)]
pub(crate) struct BcodeAuthBackend;

#[async_trait::async_trait(?Send)]
impl AuthBackend for BcodeAuthBackend {
    fn scope_key(&self, config: &BcodeComConfig) -> String {
        config.auth_scope()
    }

    /// Devbox auth files from before the OIDC flow wrote this key, and only this backend ever minted credentials into it.
    fn inherited_scopes(&self) -> &'static [&'static str] {
        &[crate::auth::model::LEGACY_SCOPE]
    }

    /// An bcode login can come from OAuth2, a customer's own login provider, the auth binary, or a devbox, so there is no one issuer to check for.
    /// Saying yes to all of them is safe: a credential minted elsewhere still gets sent to bcode, which rejects it.
    fn owns(&self, _auth: &BcodeAuth) -> bool {
        true
    }

    /// Only a first-party endpoint may receive the session bearer.
    ///
    /// Upstream returned `true` for every URL, which was safe while every model
    /// in the catalog was its own. This catalog is multi-provider: without this
    /// gate an OAuth session would ride along to `api.deepseek.com` or
    /// `api.openai.com` on the next request. Provider credentials come from the
    /// model's own `api_key`/`env_key` instead.
    fn may_receive_session(&self, url: &str) -> bool {
        crate::util::is_bcode_api_bearer_url(url)
    }

    fn login_host(&self, config: &BcodeComConfig) -> String {
        super::host_of(&config.bcode_ws_origin)
    }

    fn is_bcode_authority(&self) -> bool {
        true
    }

    async fn login(&self, req: LoginRequest<'_>) -> anyhow::Result<(BcodeAuth, bool)> {
        crate::auth::flow::run_auth_flow_steps(
            req.auth_manager,
            req.bcode_com_config,
            req.reauth,
            req.force_interactive,
            req.on_stderr,
            req.url_tx,
            req.code_rx,
            req.login_override,
        )
        .await
    }

    fn refresher(
        &self,
        manager: Arc<AuthManager>,
        auth_provider_command: Option<String>,
        diagnostic_uploader: Option<DiagnosticUploader>,
    ) -> Arc<dyn TokenRefresher> {
        match auth_provider_command {
            Some(cmd) => {
                let runner: Arc<dyn ExternalCommandRunner> = manager;
                Arc::new(ExternalBinaryRefresher::new(runner, cmd))
            }
            None => {
                let snapshot: Arc<dyn AuthSnapshot> = manager;
                let refresher = OidcRefresher::new(snapshot);
                match diagnostic_uploader {
                    Some(uploader) => Arc::new(refresher.with_diagnostic_upload(uploader)),
                    None => Arc::new(refresher),
                }
            }
        }
    }
}
