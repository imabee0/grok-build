// Per-test-case module for the `pty_e2e` integration test crate.
#[allow(unused_imports)]
use super::common::*;

/// A cold boot with no API key must show the first-run provider picker
/// (logo + catalog names + Quit), not the authenticated welcome menu.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn first_run_shows_provider_picker_with_logo_and_quit() {
    let content = ContentController::start().await.expect("start content");

    let binary = pager_binary().expect("resolve pager binary");
    let mut harness = PtyHarness::spawn_with_content_env_ops(
        &binary,
        DEFAULT_ROWS,
        DEFAULT_COLS,
        &content,
        &[],
        &[EnvOp::remove("BCODE_API_KEY")],
    )
    .expect("spawn pager");

    harness
        .wait_for_text("Sign in to a provider", WELCOME_TIMEOUT)
        .expect("first-run title");

    let screen = harness.screen_contents();
    assert!(
        screen.contains("Quit"),
        "first-run must offer Quit:\n{screen}"
    );
    assert!(
        screen.contains("Providers"),
        "expected Providers tab:\n{screen}"
    );
    assert!(
        screen.contains('⣷') || screen.contains('⣿'),
        "expected the existing bcode mark:\n{screen}"
    );
    assert!(
        !screen.contains("Login with"),
        "must not offer a bcode account login:\n{screen}"
    );

    harness.quit().expect("clean quit");
}
