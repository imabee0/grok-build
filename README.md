# bcode

A terminal AI coding agent. Full-screen TUI, subagents, hooks, skills,
sandboxing, and headless mode for scripting — with the model provider left
entirely up to you.

bcode is a downstream fork of an Apache-2.0 upstream (credited in
[`NOTICE`](NOTICE)), rebranded and made provider-neutral. DeepSeek, OpenAI and
others are peers: none is privileged, none is assumed, and nothing phones home.

> Status: early. The fork's structure and build are in place; the
> multi-provider, multi-account, pricing and usage work is in progress.

## Why

Existing terminal agents force a trade: a good harness tied to one vendor, or a
provider-neutral tool with a weaker harness. bcode takes a strong harness and
removes the vendor coupling.

- **Any provider.** OpenAI Chat Completions, OpenAI Responses, and Anthropic
  Messages wire formats, so most endpoints work with a config block.
- **Several accounts at once.** Not "switch accounts" — multiple credentials
  live simultaneously, and different subagents can run on different ones.
- **Honest usage.** Session tokens split cached vs uncached, and spend priced
  locally per call at the rate in force when the call was made.
- **Quiet by default.** No telemetry, no announcements, no update check, no
  remote settings. The only network calls are the ones your provider needs.
- **Stays out of the way.** bcode writes under `~/.bcode` and nowhere else. An
  existing agent CLI on the same machine is left byte-identical.

## Building

Requires the Rust toolchain pinned by [`rust-toolchain.toml`](rust-toolchain.toml)
(rustup installs it on first build) and `protoc` on `PATH`.

```sh
cargo build -p bcode-pager-bin --release   # -> target/release/bcode
cargo run -p bcode-pager-bin               # build and launch the TUI
make verify                                # the ship gate
```

The workspace is 94 crates and a full build is slow. Scope your work:
`cargo check -p <crate>`.

## Layout

| Path | Contents |
| --- | --- |
| `crates/codegen/bcode-pager-bin` | Composition root; builds the `bcode` binary |
| `crates/codegen/bcode-pager` | TUI: scrollback, prompt, modals, status line |
| `crates/codegen/bcode-shell` | Agent runtime, auth, sampling, sessions |
| `crates/codegen/bcode-models` | Provider registry and model catalog (data) |
| `crates/codegen/bcode-tools` | Tool implementations |
| `tools/` | Rebrand codemod, brand gate, upstream sync |
| `third_party/` | Vendored upstream source (Mermaid diagram stack) |

## How the fork tracks upstream

Upstream publishes roughly one squashed snapshot commit per day, so a
merge-based fork would re-resolve a 2,000-file rename on every drop. Instead:

```
vendor ──────────●───────────●        pristine upstream snapshots
                  \           \
                   ● rebrand   ● rebrand    regenerated, never merged
                    \           \
                     ●─●─●       ●─●─●      feature commits, rebased
```

`make sync` fetches a new snapshot, regenerates the rebrand commit from
[`tools/rebrand.toml`](tools/rebrand.toml), replays the feature stack, and runs
the verify gate. Because the rebrand is regenerated rather than merged forward,
it cannot conflict — only the small feature stack rebases.

See [`AGENTS.md`](AGENTS.md) for the working agreement and invariants.

## License

Apache-2.0 — see [`LICENSE`](LICENSE). Upstream copyright and the change notice
required by Apache-2.0 §4(b) are in [`NOTICE`](NOTICE). Third-party and vendored
code remains under its original licenses; see
[`THIRD-PARTY-NOTICES`](THIRD-PARTY-NOTICES) and
[`third_party/NOTICE`](third_party/NOTICE).

bcode is not affiliated with, endorsed by, or sponsored by any model provider it
can be configured to use.
