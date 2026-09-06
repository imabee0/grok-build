//! Binary resolution, serial env guards, and git sandbox creation.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::sandbox::TestSandbox;

/// Parse env var `key` into `T`, falling back to `default` when it is unset or present-but-unparseable (warning in the latter case).
pub fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    let Ok(raw) = std::env::var(key) else {
        return default;
    };
    match raw.parse() {
        Ok(value) => value,
        Err(_) => {
            eprintln!("[test-support] ignoring unparseable {key}={raw:?}; using default");
            default
        }
    }
}

/// RAII guard for a single environment variable in `#[serial]` tests.
/// It snapshots the prior value, applies the change, and restores the prior value (or unsets it) on drop, even if an assertion panics.
/// Restoring rather than always unsetting avoids clobbering vars a parent process/harness set (e.g. `RUST_LOG`).
///
/// Callers MUST be `#[serial_test::serial]`.
/// The `unsafe` `set_var`/`remove_var` are sound only when no other thread accesses the environment concurrently.
pub struct EnvGuard {
    key: &'static str,
    prior: Option<OsString>,
}

impl EnvGuard {
    /// Set `key` to `value` for the guard's lifetime.
    pub fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let prior = std::env::var_os(key);
        // SAFETY: callers are `#[serial]`, so no other thread touches the env.
        unsafe { std::env::set_var(key, value) };
        Self { key, prior }
    }

    /// Unset `key` for the guard's lifetime.
    pub fn unset(key: &'static str) -> Self {
        let prior = std::env::var_os(key);
        // SAFETY: see [`EnvGuard::set`].
        unsafe { std::env::remove_var(key) };
        Self { key, prior }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: see [`EnvGuard::set`].
        match self.prior.take() {
            Some(v) => unsafe { std::env::set_var(self.key, v) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

/// # Safety
/// No other thread may access the environment concurrently; call before any other thread exists.
pub unsafe fn isolate_bcode_env(home: &Path) {
    // SAFETY: forwarded to the caller.
    unsafe {
        std::env::set_var("BCODE_HOME", home);
        std::env::set_var("BCODE_TELEMETRY_ENABLED", "false");
        std::env::set_var("BCODE_FEEDBACK_ENABLED", "false");
        std::env::set_var("BCODE_TRACE_UPLOAD", "false");
        for var in [
            "BCODE_DEPLOYMENT_KEY",
            "BCODE_MANAGED_CONFIG",
            "BCODE_CONFIG",
            "BCODE_CONFIG_PATH",
            "BCODE_CLI_CHAT_PROXY_BASE_URL",
            "BCODE_MODELS_BASE_URL",
            "BCODE_MODELS_LIST_URL",
            "BCODE_API_KEY",
            "BCODE_API_KEY",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            std::env::remove_var(var);
        }
    }
}

fn workspace_root() -> PathBuf {
    // nth(3): crate is nested three levels below the cargo workspace root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("workspace root")
        .to_path_buf()
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"))
}

fn local_bcode_binary_path() -> PathBuf {
    target_dir()
        .join("debug")
        .join(format!("bcode-pager{}", std::env::consts::EXE_SUFFIX))
}

fn ensure_local_bcode_binary(binary: &Path) {
    if binary.exists() {
        return;
    }

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut cmd = Command::new(&cargo);
    cmd.current_dir(workspace_root())
        .args(["build", "-p", "bcode-pager-bin", "--bin", "bcode-pager"])
        .stdin(std::process::Stdio::null())
        .envs(bcode_tty_utils::pager_env());
    bcode_tty_utils::detach_std_command(&mut cmd);
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {cargo} to build bcode-pager: {e}"));

    assert!(
        output.status.success(),
        "failed to build bcode-pager for lifecycle tests (exit {:?})\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        binary.exists(),
        "bcode-pager build completed but binary missing at {}",
        binary.display()
    );
}

/// Resolve bcode binary: `BCODE_BINARY` env (CI) or a locally built `bcode-pager` binary.
pub fn bcode_binary() -> PathBuf {
    if let Ok(path) = std::env::var("BCODE_BINARY") {
        let p = PathBuf::from(path);
        assert!(p.exists(), "BCODE_BINARY does not exist: {}", p.display());
        // Bazel's BCODE_BINARY is runfiles-relative; the harness spawns the child with a different cwd
        // Absolutize against the (runfiles-root) cwd now
        return std::path::absolute(&p).unwrap_or(p);
    }

    if let Ok(path) = std::env::var("CARGO_BIN_EXE_bcode") {
        let p = PathBuf::from(path);
        if p.exists() {
            return p;
        }
    }

    let binary = local_bcode_binary_path();
    ensure_local_bcode_binary(&binary);
    binary
}

pub fn git_workdir() -> TestSandbox {
    TestSandbox::builder().git().build()
}
