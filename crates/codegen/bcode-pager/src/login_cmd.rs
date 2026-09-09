//! `bcode login` / `bcode logout`: sign in to a *provider*, not a product.
//!
//! bcode has no backend of its own to sign into -- the catalog's providers do.
//! With no `--oauth`/`--device-auth` flag (enterprise SSO, unchanged), this is
//! a small stdin wizard over [`bcode_shell::auth::provider_setup`], the same
//! credential policy the in-TUI provider manager uses: pick a provider from
//! the catalog, read its key off stdin, store it at the `provider::<id>`
//! scope in `auth.json` so every model on that provider works with nothing
//! written to `config.toml`. The CLI and the TUI call the same policy layer
//! so they cannot drift on what counts as a valid id or an empty key.

use std::io::Write;

use anyhow::{Context, Result, bail};
use bcode_shell::auth::accounts;
use bcode_shell::auth::provider_setup;

#[derive(Debug, clap::Args, Clone)]
pub struct LoginArgs {
    /// Provider id to sign in to (see `bcode login` with no id for the list).
    pub provider: Option<String>,
    /// Store the key under this account name (`bcode account add`) instead of
    /// the provider-wide credential. Useful for a second key on one provider.
    #[arg(long)]
    pub account: Option<String>,
    /// Read the key from this environment variable instead of prompting.
    #[arg(long)]
    pub from_env: Option<String>,
    /// Enterprise SSO: force the loopback-browser flow. Unrelated to provider
    /// sign-in; ignores `provider`/`account`/`from_env` when set.
    #[arg(long = "oauth", alias = "oidc", conflicts_with_all = ["device_auth"])]
    pub oauth: bool,
    /// Enterprise SSO: force the RFC 8628 device-code flow. Unrelated to
    /// provider sign-in; ignores `provider`/`account`/`from_env` when set.
    #[arg(
        long = "device-auth",
        visible_alias = "device-code",
        conflicts_with_all = ["oauth"]
    )]
    pub device_auth: bool,
}

#[derive(Debug, clap::Args, Clone, Default)]
pub struct LogoutArgs {
    /// Remove this provider's stored credential.
    pub provider: Option<String>,
    /// Remove this account's stored credential.
    #[arg(long)]
    pub account: Option<String>,
    /// Clear every provider credential, every account credential, and the
    /// enterprise SSO session.
    #[arg(long)]
    pub all: bool,
}

/// `bcode login [PROVIDER]`.
///
/// `--oauth`/`--device-auth` are the pre-existing enterprise-SSO transports
/// and run unchanged; everything else is the provider wizard.
pub async fn run(config: &bcode_shell::agent::config::Config, args: LoginArgs) -> Result<()> {
    if args.oauth || args.device_auth {
        bcode_shell::auth::run_cli_login(config, args.oauth, args.device_auth).await?;
        println!();
        return Ok(());
    }
    let home = bcode_shell::util::bcode_home::bcode_home();
    let provider = match args.provider {
        Some(id) => lookup_provider(&id)?,
        None => pick_provider_interactively()?,
    };
    // A provider with its own OAuth app (e.g. ChatGPT sign-in) has no API key
    // to paste: run the loopback-browser flow instead of the stdin key wizard.
    // `--account`/`--from-env` don't apply to it.
    if let Some(auth) = provider.auth.as_ref() {
        bcode_shell::auth::oidc::run_provider_oauth_login(&provider.id, &provider.name, auth, None)
            .await?;
        println!();
        offer_default_model(&provider);
        return Ok(());
    }
    let key = match args.from_env.as_deref() {
        Some(var) => {
            std::env::var(var).with_context(|| format!("environment variable {var} is not set"))?
        }
        None => crate::account_cmd::read_key_from_stdin(&provider.name)?,
    };
    let key = key.trim();
    if key.is_empty() {
        bail!(
            "refusing to store an empty key for provider {:?}",
            provider.id
        );
    }
    match args.account.as_deref() {
        Some(account) => {
            provider_setup::store_account_credential(&home, account, key)
                .with_context(|| format!("failed to store the key for account {account:?}"))?;
            println!(
                "account {account}: key stored in {}",
                home.join("auth.json").display()
            );
            println!("point a model at it with:\n\n    [model.<id>]\n    account = \"{account}\"");
        }
        None => {
            provider_setup::store_provider_credential(&home, &provider.id, key).with_context(
                || format!("failed to store the key for provider {:?}", provider.id),
            )?;
            println!(
                "{}: key stored in {} -- every {} model now has a credential",
                provider.name,
                home.join("auth.json").display(),
                provider.name
            );
            offer_default_model(&provider);
        }
    }
    Ok(())
}

/// `bcode logout [PROVIDER] [--account NAME] [--all]`.
///
/// No arguments clears the enterprise-SSO session, same as before this
/// command grew provider awareness.
pub fn run_logout(config: &bcode_shell::agent::config::Config, args: LogoutArgs) -> Result<()> {
    if args.all {
        let home = bcode_shell::util::bcode_home::bcode_home();
        let mut cleared = Vec::new();
        for id in accounts::stored_provider_ids(&home) {
            if provider_setup::remove_provider_credential(&home, &id).unwrap_or(false) {
                cleared.push(format!("provider {id}"));
            }
        }
        for name in accounts::stored_account_names(&home) {
            if provider_setup::remove_account_credential(&home, &name).unwrap_or(false) {
                cleared.push(format!("account {name}"));
            }
        }
        bcode_shell::auth::run_cli_logout(config)?;
        if cleared.is_empty() {
            println!("no stored provider or account credentials to clear");
        } else {
            println!("cleared: {}", cleared.join(", "));
        }
        return Ok(());
    }
    if let Some(account) = args.account.as_deref() {
        let home = bcode_shell::util::bcode_home::bcode_home();
        if provider_setup::remove_account_credential(&home, account)
            .with_context(|| format!("failed to remove the key for account {account:?}"))?
        {
            println!("account {account}: stored key removed");
        } else {
            println!("account {account}: no stored key");
        }
        return Ok(());
    }
    if let Some(provider) = args.provider.as_deref() {
        let home = bcode_shell::util::bcode_home::bcode_home();
        let id = lookup_provider(provider)?.id;
        if provider_setup::remove_provider_credential(&home, &id)
            .with_context(|| format!("failed to remove the key for provider {id:?}"))?
        {
            println!("{provider}: stored key removed");
        } else {
            println!("{provider}: no stored key");
        }
        return Ok(());
    }
    bcode_shell::auth::run_cli_logout(config)
}

fn lookup_provider(id: &str) -> Result<bcode_models::ProviderInfo> {
    bcode_models::provider(id).cloned().ok_or_else(|| {
        let known: Vec<&str> = bcode_models::providers()
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        anyhow::anyhow!(
            "unknown provider {id:?}; known providers: {}",
            known.join(", ")
        )
    })
}

/// Print the numbered picker and read a choice from stdin. Refuses rather
/// than hangs when stdin is not a terminal: a script must pass a provider id
/// explicitly, since the key itself also has to come off the one stdin pipe.
fn pick_provider_interactively() -> Result<bcode_models::ProviderInfo> {
    let providers = bcode_models::providers();
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let known: Vec<&str> = providers.iter().map(|p| p.id.as_str()).collect();
        bail!(
            "no provider given and stdin is not a terminal; run `bcode login <provider>` \
             (one of: {}), or pass --from-env",
            known.join(", ")
        );
    }
    eprintln!("Sign in to a provider\n");
    for (i, p) in providers.iter().enumerate() {
        eprintln!("  {}. {}\t{}", i + 1, p.name, host_of(&p.base_url));
    }
    eprint!("\nProvider number or id: ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("failed to read the provider choice from stdin")?;
    let choice = line.trim();
    if let Ok(n) = choice.parse::<usize>()
        && n >= 1
        && n <= providers.len()
    {
        return Ok(providers[n - 1].clone());
    }
    lookup_provider(choice)
}

fn host_of(base_url: &str) -> String {
    url::Url::parse(base_url)
        .ok()
        .and_then(|u: url::Url| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| base_url.to_owned())
}

/// After a successful provider sign-in, offer to set that provider's catalog
/// default as `models.default`. Only when a terminal is attached: a script
/// piping the key in already got its one stdin read, and a script has no use
/// for an interactive prompt anyway.
fn offer_default_model(provider: &bcode_models::ProviderInfo) {
    let Some(model) = bcode_models::providers()
        .iter()
        .find(|p| p.id == provider.id)
        .map(|p| p.default_model.clone())
    else {
        return;
    };
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        println!(
            "set it as your default with:\n\n    bcode models\n    [models]\n    default = \"{model}\""
        );
        return;
    }
    eprint!("\nMake {model} your default model? [Y/n] ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return;
    }
    let answer = line.trim().to_ascii_lowercase();
    if answer.is_empty() || answer == "y" || answer == "yes" {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("could not set the default model: {e}");
                return;
            }
        };
        match rt.block_on(bcode_shell::util::config::set_default_model(model.clone())) {
            Ok(()) => println!("default model set to {model}"),
            Err(e) => eprintln!("could not set the default model: {e}"),
        }
    }
}
