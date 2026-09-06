# bcode

A terminal coding agent: fullscreen TUI, subagents, ACP, hooks, skills, sandbox.
Downstream fork of an Apache-2.0 upstream (named in NOTICE), rebranded and made
provider-neutral. Every model provider is a peer; none is privileged.

## Verified commands

```sh
make verify                      # ship gate: brand + fmt + check + test. Run before every push.
make test                        # tests for the crates the fork touches
make coexist                     # other CLIs' state is byte-identical after a session
make egress                      # no network call with no provider selected
make brand                       # zero-branding guarantee on its own
make build                       # release binary
make sync                        # pull a new upstream snapshot, rebuild main
cargo check -p <crate>           # ALWAYS scope to a crate; the workspace is 94 crates and slow
cargo run -p bcode-pager-bin     # launch the TUI
```

There is no CI yet, by design. `make verify` runs locally in one foreground
shell and its output is read before any push.

## Repository shape

| Path | Contents |
| --- | --- |
| `tools/rebrand.toml` | Ordered rename rules. Single source of truth for the fork's identity. |
| `tools/rebrand.py` | Idempotent codemod: file contents + path names. |
| `tools/sync-upstream.sh` | Fetch upstream, regenerate the rebrand, replay the feature stack. |
| `tools/verify-no-upstream-brand.sh` | The brand gate. |
| `crates/codegen/bcode-pager-bin` | Composition root; builds the `bcode` binary. |
| `crates/codegen/bcode-pager` | TUI: scrollback, prompt, modals, status line. |
| `crates/codegen/bcode-shell` | Agent runtime, auth, sampling, sessions. |
| `crates/codegen/bcode-models` | Provider data: model catalog and rate card. |
| `crates/codegen/bcode-tools` | Tool implementations. |

## Branch model

- `vendor` — pristine upstream snapshots. **Never hand-edit.**
- `main` — `vendor`, then one generated `rebrand:` commit, then the feature stack.

The rebrand commit is *regenerated*, not merged forward, so it cannot conflict.
Only the feature commits rebase. Keep them small, additive, and anchored to a
single seam, or they will rot against upstream refactors.

## Invariants

- **Zero upstream branding.** `make brand` is the gate. Only two exemptions: the
  provider data in `crates/codegen/bcode-models/` — `default_models.json` and
  `pricing.toml`, both keyed by wire model id, both data, neither compiled as
  code — and the licence files (Apache-2.0 §4(c)/(d) requires retaining them).
  Tests must not name a provider's model ids: read them from the catalog.
- **bcode writes only under `~/.bcode`, the workspace, and explicit user
  targets.** Never another tool's directory. Any other agent CLI's home
  directory must be byte-identical after a bcode session.
- **No network call the user's chosen provider didn't cause.** No telemetry, no
  announcements, no update check, no remote settings, no model-registry fetch.
- Do not inherit permission rules or `defaultMode` from another tool's config.
- The root `Cargo.toml` is generated upstream. We own it downstream, but prefer
  editing per-crate manifests so the codemod stays the only thing that rewrites it.
