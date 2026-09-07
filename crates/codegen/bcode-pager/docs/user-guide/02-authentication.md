# Authentication

Bcode has no backend of its own to sign into. Your model provider does.
Authentication means giving Bcode a credential for *that* provider -- an API
key by default, or your own enterprise SSO if you run one.

---

## `bcode login` (the default path)

```bash
bcode login
```

Lists the providers in the catalog, asks you to pick one, and reads its API
key off stdin -- never as a command-line argument, so it never lands in shell
history or `ps`:

```
Sign in to a provider

  1. DeepSeek   api.deepseek.com
  2. OpenAI     api.openai.com
  3. ...        (every provider in your build's catalog)

Provider number or id: 1
Paste your DeepSeek API key: ****************
DeepSeek: key stored in ~/.bcode/auth.json -- every DeepSeek model now has a credential

Make deepseek-v4-pro your default model? [Y/n]
```

The key is stored once, under that provider's own scope in `~/.bcode/auth.json`
(owner-only, `0600`) -- not per model, so every model on that provider works
immediately with nothing written to `config.toml`. Accepting the default-model
offer writes `models.default` through the same writer `/model` uses; nothing
else in `config.toml` is touched.

Non-interactive:

```bash
bcode login deepseek --from-env DEEPSEEK_API_KEY   # read the key from the environment
echo "sk-..." | bcode login deepseek               # read the key from stdin
```

`bcode login` with no provider and no terminal attached refuses rather than
hangs -- pass a provider id explicitly in scripts.

### A second key on the same provider

`--account <name>` stores the key as a named account instead of the
provider-wide credential, so a model can request it by name:

```bash
bcode login deepseek --account ds-alt
```

```toml
[model.ds-alt]
model = "deepseek-v4-pro"
account = "ds-alt"
```

See [Custom Models](11-custom-models.md#accounts-several-credentials-at-once)
for routing subagents to different accounts.

### Sign out

```bash
bcode logout deepseek        # clear one provider's stored credential
bcode logout --account ds-alt  # clear one named account
bcode logout --all            # clear every provider, every account, and enterprise SSO
```

---

## API Key (no wizard)

Exporting the provider's own environment variable works the same as
`bcode login`, without touching `auth.json`:

```bash
export DEEPSEEK_API_KEY="sk-..."
bcode
```

Each model in the catalog names the variable its provider reads. A model's own
`api_key` or `env_key` always wins over anything `bcode login` stored -- see
[Auth Precedence](#auth-precedence).

### Credential storage

Tokens in `~/.bcode/auth.json` (and MCP OAuth tokens in
`~/.bcode/mcp_credentials.json`) are written with owner-only permissions
(`0600` on Unix). Anyone with filesystem access to those paths can use the
credentials, so:

- Prefer full-disk encryption (FileVault, BitLocker, LUKS, or equivalent).
- Do not copy `auth.json` or `mcp_credentials.json` into shared directories, tickets, or chat.
- On multi-user hosts, keep `$HOME` / `$BCODE_HOME` private to your account.

---

## Enterprise SSO

For a team running its own identity provider or auth gateway in front of
Bcode, rather than BYOK per developer. Two transports, both unrelated to
`bcode login`'s provider wizard and off by default.

### OIDC (Customer SSO)

Authenticate developers through your own Identity Provider (IdP) -- such as Okta, Azure AD, or Auth0.

#### 1. Register a public client in your IdP

- Grant type: Authorization Code with PKCE (Proof Key for Code Exchange)
- Redirect URI: `http://127.0.0.1/callback` -- a loopback address. Bcode binds a random port at sign-in time, and most IdPs treat the loopback redirect as port-agnostic per [RFC 8252](https://tools.ietf.org/html/rfc8252).
- No client secret. PKCE replaces it.

#### 2. Configure the CLI

Via config file:

```toml
# ~/.bcode/config.toml
[auth.oidc]
issuer = "https://acme.okta.com"
client_id = "0oa1b2c3d4e5f6g7h8i9"
```

Or via environment variables:

```bash
export BCODE_OIDC_ISSUER="https://acme.okta.com"
export BCODE_OIDC_CLIENT_ID="0oa1b2c3d4e5f6g7h8i9"
```

You can also override the API endpoint to point at your own proxy:

```bash
export BCODE_CLI_CHAT_PROXY_BASE_URL="https://bcode-proxy.acme.com/v1"
```

#### 3. Run `bcode login --oauth` (or just `bcode`)

Once `auth.oidc` is configured, the CLI discovers endpoints via
`{issuer}/.well-known/openid-configuration`, opens the IdP login page, and
stores tokens in `~/.bcode/auth.json`. Tokens auto-refresh silently via the
stored `refresh_token`. Without a configured issuer, `--oauth` and
`--device-auth` fall back to a built-in issuer this fork cannot reach --
configure `auth.oidc` (or `auth.oauth2`) first.

#### Optional fields

| Field | Default | Notes |
|-------|---------|-------|
| `scopes` | `["openid", "profile", "email", "offline_access", "api:access"]` | `offline_access` enables silent token refresh |
| `audience` | None | Required by some IdPs (e.g., Auth0) |

#### Device Code Flow

For headless environments (SSH sessions, Docker containers, remote VMs) where no browser is available locally:

```bash
bcode login --device-auth    # or: bcode login --device-code
```

This prints a URL and code to the terminal. Open the URL on any device, enter the code, and complete authentication. Bcode polls until the login is confirmed. Same issuer configuration as above applies.

### External Auth Provider

When browser-based login isn't possible -- for example, on sandboxed VMs, CI runners, or air-gapped networks -- delegate authentication to an external binary or script. The same contract backs a named account of `kind = "command"` (see [Custom Models](11-custom-models.md#where-an-account-sits)), so one binary can serve both a model's own `auth_provider` and an account's.

#### How It Works

```
+--------------+     sh -c     +------------------------+
|     Bcode     |-------------->|  your auth binary      |
|              |               |                        |
|  reads       |<-- stdout ----|  prints token          |
|  auth.json   |               |                        |
|              |   (stderr)    |  prints status/URLs    |--> surfaced to user
+--------------+               +------------------------+
```

1. Bcode runs your command via `sh -c "<command>"`
2. Your binary runs whatever auth flow it needs (SSO, device code, certificate exchange)
3. **stderr** carries human-readable output, such as login URLs and status messages. Bcode reads stderr and surfaces it to the user; in the TUI, it turns the first `https://` URL into a clickable sign-in link.
4. **stdout** is captured by Bcode and saved as the access token
5. Exit 0 = success; exit non-zero = Bcode falls back to interactive login

#### The stdout / stderr Contract

| Stream | What to print | Who sees it |
|--------|---------------|-------------|
| **stdout** | The token -- nothing else | Bcode (parsed and stored in auth.json) |
| **stderr** | Login URLs, status messages, errors | The user (Bcode reads stderr and shows the sign-in URL as a clickable link in the TUI) |

**Do not print anything to stdout except the token.** No progress messages, no debug output. Bcode reads stdout, trims surrounding whitespace, and parses the result as a token.

#### stdout Token Format

**Bare string** -- just the raw token:

```
eyJhbGciOiJSUzI1NiIs...
```

**JSON** -- with optional refresh token, expiry, and issuer:

```json
{"access_token": "eyJhbGciOi...", "refresh_token": "ref-tok", "expires_in": 3600, "issuer": "https://idp.example.com"}
```

Use JSON if your tokens expire and you want Bcode to automatically re-run the binary before expiry.

JSON fields:

| Field | Required | Meaning |
|-------|----------|---------|
| `access_token` | yes | Bearer token Bcode sends to the bcode API |
| `refresh_token` | no | Stored for reference. Bcode refreshes by re-running your binary, not with an OAuth refresh grant |
| `expires_in` | no | Token lifetime in seconds; enables proactive refresh before expiry |
| `issuer` | no | Identifies the token's issuer |

#### Configuration

Via config file:

```toml
# ~/.bcode/config.toml
[auth]
auth_provider_command = "/usr/local/bin/my-auth-provider"
auth_provider_label = "Acme Corp"   # optional -- customizes the TUI login button
auth_token_ttl = 3600               # optional -- token lifetime in seconds
```

Or via environment variables:

```bash
export BCODE_AUTH_PROVIDER_COMMAND="/usr/local/bin/my-auth-provider"
export BCODE_AUTH_PROVIDER_LABEL="Acme Corp"
export BCODE_AUTH_TOKEN_TTL=3600
```

Or as a named account any model can point at:

```toml
[auth_provider.acme]
command = "/usr/local/bin/my-auth-provider"

[accounts.acme]
kind = "command"
auth_provider = "acme"

[model.deepseek-v4-pro]
account = "acme"
```

#### Token Refresh

Bcode runs your binary on two different contracts, and `BCODE_AUTH_EXPIRED` is how
it tells them apart. Each run fully replaces the stored credential, so emit the
same JSON fields (such as `issuer`) on every invocation, including refreshes.

- **`BCODE_AUTH_EXPIRED=1` — a headless refresh.** Bcode is re-minting over a
  credential it already holds: a near-expiry rotation, or a token the server
  rejected. Nobody is watching. stdin is closed, your stderr is swallowed, and
  the binary is given a few seconds before it is killed. Mint silently or exit
  non-zero — never block.
- **Unset — a sign-in.** `bcode login --oauth`/`--device-auth`, the sign-in
  screen, or the escalation Bcode performs when a headless run couldn't mint.
  A user is waiting, your stderr reaches them, and you have 300 seconds —
  enough for a browser round trip or a device code.

```bash
#!/bin/sh
if [ "$BCODE_AUTH_EXPIRED" = "1" ]; then
    # Headless: silent refresh only. Declining is the fast, correct answer
    # when your SSO session has lapsed and only the user can renew it.
    echo "Refreshing token..." >&2
    TOKEN=$(my-company-auth --refresh --silent) || exit 1
else
    echo "Authenticating via Acme Corp SSO..." >&2
    TOKEN=$(my-company-auth --login --interactive)
fi

if [ -z "$TOKEN" ]; then
    echo "Authentication failed" >&2
    exit 1
fi

echo "{\"access_token\": \"$TOKEN\", \"expires_in\": 3600}"
```

When the headless run can't produce a token, Bcode stops treating the stored
credential as usable and starts the sign-in flow instead — the same one you get
on a machine that has never signed in, with your binary's stderr shown, so a
device-code URL or a browser prompt reaches you. Exiting promptly on
`BCODE_AUTH_EXPIRED=1` is what makes that handover fast; a binary that blocks
instead makes you wait out the refresh timeout on every start. Mid-session, the
turn fails with a re-auth prompt and `/login` re-runs the binary interactively.

One case stays ambiguous, and only in **leader mode** (`--leader`, or
`[cli] use_leader = true`; off by default): with no credential at all, the
leader makes one extra attempt in the background just after startup, and that
run has the variable unset, like a sign-in. A binary that mints without help
(service account, keytab, mounted token) succeeds there and the session heals
itself. One that must prompt just sits, up to the 300s sign-in ceiling —
nothing waits on it, the sign-in screen is already up, and that run's stderr
goes to `~/.bcode/leader.log` rather than to you.

#### Environment Variables

| Variable | Description |
|----------|--------------|
| `BCODE_AUTH_PROVIDER_COMMAND` | Path to your auth binary |
| `BCODE_AUTH_PROVIDER_LABEL` | Display name on the TUI login screen (e.g., "Acme Corp") |
| `BCODE_AUTH_TOKEN_TTL` | Token lifetime in seconds (for bare-string tokens without `expires_in`) |
| `BCODE_AUTH_EXPIRED` | Set to `1` on a headless refresh: don't prompt, and don't hand back a cached token. Unset on a sign-in, where a user is attached |
| `BCODE_AUTH_EARLY_INVALIDATION_SECS` | Seconds before expiry to proactively refresh (default: 300) |

---

## Automatic Credential Refresh

Bcode automatically refreshes expired credentials:

- **Before expiry:** If your auth provider returned `expires_in` (JSON output) or you set `auth_token_ttl`, Bcode re-runs the auth binary ~5 minutes before expiry.
- **On auth error:** If the server returns 401 Unauthorized, Bcode refreshes the credentials and retries the request.
- **OIDC:** If a `refresh_token` is available, Bcode silently refreshes via your IdP without re-opening the browser.

Tune the refresh buffer:

```bash
# Refresh 5 minutes before expiry (default)
export BCODE_AUTH_EARLY_INVALIDATION_SECS=300

# Disable the proactive buffer: refresh at expiry or on a 401 (set to 0)
export BCODE_AUTH_EARLY_INVALIDATION_SECS=0
```

---

## Hot Reload

Bcode picks up changes to `~/.bcode/auth.json` automatically. If you update credentials externally (for example, with a script that writes new tokens), Bcode uses the new credentials on the next API call without a restart.

---

## Auth Precedence

Bcode resolves credentials for each request in this order, highest to lowest:

1. **The model's own `api_key` or `env_key`** -- set under `[model.<name>]` in `config.toml`. Wins whenever present.
2. **A named account** -- `[model.<name>] account = "<name>"`, resolved from that account's `env_key` or its stored key.
3. **The provider-wide credential from `bcode login`** -- matched by the model's provider, so one sign-in covers every model on it.
4. **An `auth_provider` command's cached token** -- the model's own, or its account's.
5. **The enterprise-SSO session token** -- obtained through `--oauth`/`--device-auth` and stored in `~/.bcode/auth.json`. Only ever sent to first-party bcode endpoints, so it never applies to a BYOK model.
6. **`BCODE_API_KEY`** -- global fallback.

A model naming an account or provider that resolves to nothing gets **no**
credential rather than falling through further down the list, and
`bcode inspect` reports it: billing an identity you did not name is worse than
failing.

During a session, whichever tier resolved handles that request's refresh on its own terms (proactive for an `auth_provider`, silent for OIDC, none for a static key).

---

## Related settings

Coding-data sharing — **Coding data, retention, and training** in Settings,
which `/privacy` opens — does not change these config knobs:

| Setting | How to set it |
|---------|---------------|
| `[features] telemetry` | `config.toml` or `BCODE_TELEMETRY_ENABLED` |
| `[telemetry] trace_upload` | `config.toml` or `BCODE_TELEMETRY_TRACE_UPLOAD` |
| External OpenTelemetry | `BCODE_EXTERNAL_OTEL` / `[telemetry] otel_*`. See [Monitoring Usage](24-monitoring-usage.md). |

On team accounts, only a team admin can change coding-data sharing.
Team admins can also enable or disable Zero Data Retention (ZDR) for their team.
See [How to enable ZDR](https://docs.bcode.invalid/developers/faq/security#how-to-enable-zdr).
When ZDR is on, coding-data sharing cannot be changed at all — the settings
row shows `ZDR` in place of the value. ZDR does not turn off external OTEL
or `user.email` — see [ZDR and this stream](24-monitoring-usage.md#zdr-and-this-stream).

See [Monitoring Usage](24-monitoring-usage.md#related-settings) and [Configuration](05-configuration.md#telemetry).

---

## Troubleshooting

### Debug logging

Set `RUST_LOG` to control the verbosity of the file log and headless stderr output. (The TUI's on-screen tracing pane uses a fixed filter and ignores `RUST_LOG`.) In the TUI, file logging defaults to `DEBUG`; in headless mode (`-p`), `RUST_LOG` defaults to `off` so only the answer is printed — set `RUST_LOG=error` (or broader) to see logs on stderr.

In the TUI, set `BCODE_LOG_FILE` to an absolute path to write logs to that file:

```bash
BCODE_LOG_FILE=/tmp/bcode.log RUST_LOG=debug bcode
tail -f /tmp/bcode.log
```

`BCODE_LOG_FILE` is treated as a literal file path. A relative value such as `1` writes a file named `1` in the current directory.

In headless mode, logs go to stderr. Redirect them to a file:

```bash
RUST_LOG=debug bcode -p "hello" 2> /tmp/bcode.log
```

### Common log messages

| Log message | What it means |
|-------------|---------------|
| `auth: running external auth provider (headless refresh)` / `(interactive login)` | Bcode is running your binary, and on which contract |
| `auth: external auth provider returned fresh token` | Bcode parsed and stored the token |
| `auth: external auth provider failed` | Binary exited non-zero or stdout was empty |
| `auth: external auth provider timed out (likely needs interactive auth), killing` | Binary did not exit before the timeout and was killed |
| `auth: failed to start external auth provider` | Command could not be spawned (binary not found) |

### Common fixes

- **First launch shows a provider list instead of starting** -- normal: there is no credential yet. Run `bcode login` in a terminal, or export the provider's env var.
- **"unknown provider"** -- run `bcode login` with no argument to see the current catalog list.
- **A 401 says "No credential for provider 'x'. Run `bcode login x`."** -- the model resolved no credential at all (no static key, no account, no `bcode login` credential, no `auth_provider`, no session). This is the common case, not a rejected key -- sign in and retry.
- **A stored key isn't picked up** -- a model's own `api_key`/`env_key`, and any account it names, both win over the provider-wide `bcode login` credential; see [Auth Precedence](#auth-precedence).
- **OIDC redirect fails** -- Ensure your IdP allows loopback redirect URIs (`http://127.0.0.1/callback`).
- **External auth provider not found** -- Check that the `auth_provider_command` path is correct and the binary is executable.
