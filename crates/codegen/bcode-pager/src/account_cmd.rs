//! `bcode account add|list|remove`: named provider credentials.
//!
//! An account is one credential, named, so a model or a subagent role can point
//! at it with `account = "<name>"`. Several are live at once — this command
//! manages the stored keys; nothing here switches a global identity.

use std::io::Write;

use anyhow::{Context, Result, bail};
use bcode_shell::auth::accounts;

#[derive(Debug, clap::Args, Clone)]
pub struct AccountArgs {
    #[command(subcommand)]
    pub command: AccountCommand,
}

#[derive(Debug, clap::Subcommand, Clone)]
pub enum AccountCommand {
    /// Store an API key for an account
    Add {
        /// Account name, as used by `account = "<name>"` in config.toml
        name: String,
        /// Read the key from this environment variable instead of prompting.
        /// The value is read once and stored; to avoid storing it at all, put
        /// `env_key` in the account's config table instead.
        #[arg(long)]
        from_env: Option<String>,
    },
    /// List accounts: those configured, those with a stored key, or both
    List {
        /// Emit machine-readable JSON output.
        #[arg(long)]
        json: bool,
    },
    /// Delete an account's stored key
    Remove {
        /// Account name
        name: String,
    },
}

pub fn run(args: AccountArgs) -> Result<()> {
    let home = bcode_shell::util::bcode_home::bcode_home();
    match args.command {
        AccountCommand::Add { name, from_env } => add(&home, &name, from_env.as_deref()),
        AccountCommand::List { json } => list(&home, json),
        AccountCommand::Remove { name } => remove(&home, &name),
    }
}

fn add(home: &std::path::Path, name: &str, from_env: Option<&str>) -> Result<()> {
    if !accounts::is_valid_account_name(name) {
        bail!(
            "invalid account name {name:?}: letters, digits, '_', '-' and '.' only, \
             not starting with '.'"
        );
    }
    let key = match from_env {
        Some(var) => {
            std::env::var(var).with_context(|| format!("environment variable {var} is not set"))?
        }
        None => read_key_from_stdin(name)?,
    };
    if key.trim().is_empty() {
        bail!("refusing to store an empty key for account {name:?}");
    }
    accounts::store_account_key(home, name, key.trim())
        .with_context(|| format!("failed to store the key for account {name:?}"))?;
    // The key itself is never echoed back, here or anywhere else.
    println!(
        "account {name}: key stored in {}",
        home.join("auth.json").display()
    );
    println!("point a model at it with:\n\n    [model.<id>]\n    account = \"{name}\"");
    Ok(())
}

/// Read a key from stdin.
///
/// A pipe (`echo $KEY | bcode account add ds-main`) is the scriptable path. An
/// interactive terminal is prompted, and the key still comes off stdin rather
/// than an argument, so it never lands in the shell history or in `ps`.
pub(crate) fn read_key_from_stdin(name: &str) -> Result<String> {
    use std::io::BufRead;
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        // The terminal still echoes what is typed; what this buys is that the
        // key never reaches argv, so it stays out of shell history and `ps`.
        eprint!("API key for account {name} (read from stdin, not from the command line): ");
        std::io::stderr().flush().ok();
    }
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("failed to read the key from stdin")?;
    Ok(line)
}

fn list(home: &std::path::Path, json: bool) -> Result<()> {
    let config = bcode_shell::config::load_agent_config_disk_only()
        .map_err(|e| anyhow::anyhow!("failed to load config: {e}"))?;
    let stored = accounts::stored_account_names(home);
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut names: Vec<&str> = config.accounts.keys().map(String::as_str).collect();
    for name in &stored {
        if !config.accounts.contains_key(name.as_str()) {
            names.push(name.as_str());
        }
    }
    names.sort_unstable();
    names.dedup();
    for name in names {
        let configured = config.accounts.get(name);
        rows.push(serde_json::json!({
            "name": name,
            "kind": configured.map_or("api-key", |a| a.kind.as_str()),
            "configured": configured.is_some(),
            "envKey": configured.and_then(|a| a.env_key.as_ref().map(|k| k.names().join(", "))),
            "hasStoredKey": stored.iter().any(|s| s == name),
            "description": configured.and_then(|a| a.description.clone()),
        }));
    }
    let mut out = std::io::stdout().lock();
    if json {
        writeln!(out, "{}", serde_json::to_string_pretty(&rows)?)?;
        return Ok(());
    }
    if rows.is_empty() {
        writeln!(
            out,
            "no accounts. Declare one with:\n\n    [accounts.<name>]\n    kind = \"api-key\"\n\
             \nthen store its key with `bcode account add <name>`."
        )?;
        return Ok(());
    }
    for row in &rows {
        let name = row["name"].as_str().unwrap_or_default();
        let kind = row["kind"].as_str().unwrap_or_default();
        let source = match (
            row["envKey"].as_str(),
            row["hasStoredKey"].as_bool() == Some(true),
        ) {
            (Some(env), true) => format!("{env}, else stored key"),
            (Some(env), false) => format!("{env} (no stored key)"),
            (None, true) => "stored key".to_owned(),
            (None, false) => "no credential".to_owned(),
        };
        let unconfigured = if row["configured"].as_bool() == Some(true) {
            ""
        } else {
            "  [no [accounts.*] table]"
        };
        writeln!(out, "{name}  ({kind})  {source}{unconfigured}")?;
    }
    Ok(())
}

fn remove(home: &std::path::Path, name: &str) -> Result<()> {
    if accounts::remove_account_key(home, name)
        .with_context(|| format!("failed to remove the key for account {name:?}"))?
    {
        println!("account {name}: stored key removed");
    } else {
        println!("account {name}: no stored key");
    }
    Ok(())
}
