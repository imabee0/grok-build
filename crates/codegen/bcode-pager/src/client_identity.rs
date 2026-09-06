pub const PAGER_CLIENT_TYPE: &str = "bcode-pager";
pub const HEADLESS_CLIENT_TYPE: &str = "bcode-shell";

pub const PAGER_CLIENT_VERSION: &str = bcode_version::VERSION;

/// `User-Agent` for the pager's own HTTP clients that call `api.bcode.invalid` directly (voice STT).
///
/// Matches the sampler's `bcode-shell/<version> (os; arch)` shape so server-side dashboards bucket voice traffic alongside chat / imagine requests.
pub fn client_user_agent() -> String {
    format!(
        "{}/{} ({}; {})",
        HEADLESS_CLIENT_TYPE,
        PAGER_CLIENT_VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_user_agent_has_expected_shape() {
        // e.g. "bcode-shell/1.2.3 (macos; aarch64)".
        // Servers parse this UA string, so pin the exact shape
        let ua = client_user_agent();
        assert_eq!(
            ua,
            format!(
                "bcode-shell/{} ({}; {})",
                PAGER_CLIENT_VERSION,
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        );
    }
}
