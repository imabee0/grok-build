//! This test runs in its own process because the assertions consume process-global state.

#[test]
fn prefetched_agent_id_resolves_and_persists() {
    let home = tempfile::tempdir().expect("tempdir");
    // SAFETY: single-threaded here; set before anything caches `bcode_home()`.
    unsafe {
        std::env::remove_var("BCODE_AGENT_ID");
        std::env::set_var("BCODE_HOME", home.path());
    }
    bcode_telemetry::id::prefetch_agent_id();
    let id = bcode_telemetry::id::agent_id();
    assert_eq!(
        std::fs::read_to_string(home.path().join("agent_id"))
            .expect("agent_id cache")
            .trim(),
        id
    );
}
