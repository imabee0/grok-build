//! `BCODE_HOME` override tests in an isolated binary so `bcode_home()`'s process-wide `OnceLock` initializes from the overridden env var.

use std::path::PathBuf;

#[test]
#[serial_test::serial(BCODE_HOME)]
fn bcode_home_override_path_helpers() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bcode_home = tmp.path().to_path_buf();
    unsafe {
        std::env::set_var("BCODE_HOME", &bcode_home);
    }

    assert_eq!(
        bcode_pager::util::pager_toml_path(),
        bcode_home.join("pager.toml")
    );
    assert_eq!(
        bcode_pager::util::display_bcode_home_prefix(),
        "$BCODE_HOME"
    );
    assert_eq!(
        bcode_pager::util::display_user_bcode_path("config.toml"),
        "$BCODE_HOME/config.toml"
    );

    let memory_path = bcode_home.join("memory/MEMORY.md");
    assert_eq!(
        bcode_pager::util::abbreviate_path(&memory_path.display().to_string()),
        "$BCODE_HOME/memory/MEMORY.md"
    );

    // The copy toast abbreviates paths the same way, so a custom $BCODE_HOME outside $HOME still shows the short form
    assert_eq!(
        bcode_pager::clipboard::display_copy_path(&bcode_home.join("last-copy.txt")),
        "$BCODE_HOME/last-copy.txt"
    );

    assert!(bcode_pager::util::is_under_user_bcode_home(&memory_path));
    assert!(!bcode_pager::util::is_under_user_bcode_home(
        PathBuf::from("/tmp/other").as_path()
    ));
}

/// Isolated because `bcode_home()`'s `OnceLock` is already initialized by the time the shared lib-test binary reaches a case like this.
#[test]
#[serial_test::serial(BCODE_HOME)]
fn disk_usage_run_creates_no_bcode_home() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ghost = tmp.path().join("ghost-home");
    unsafe {
        std::env::set_var("BCODE_HOME", &ghost);
    }

    for json in [false, true] {
        bcode_pager::disk_usage_cmd::run(bcode_pager::disk_usage_cmd::DiskUsageArgs { json })
            .expect("a missing home is not an error");
        assert!(
            !ghost.exists(),
            "bcode du must not create the home it reports on (json={json})"
        );
    }
}
