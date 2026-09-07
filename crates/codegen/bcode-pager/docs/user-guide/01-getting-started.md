# Getting Started

Bcode is a terminal coding agent. It runs as a TUI that reads your codebase, runs shell commands, edits files and manages tasks, against whichever model provider you configure.

You can use it interactively as a full-screen TUI, run it headlessly for scripting and CI/CD, or integrate it into editors via the Agent Client Protocol (ACP).

---

## Installation

Build from source. There is no hosted installer and no auto-update: the binary
you run is the one you built.

```bash
git clone <this repository> && cd bcode
make build                 # cargo build -p bcode-pager-bin --release
```

The toolchain is pinned by `rust-toolchain.toml`, and the build needs `protoc`
on `PATH`. The binary lands at `target/release/bcode`; put it wherever you keep
your own tools, or run it in place.

Verify:

```bash
target/release/bcode --version
```

To fetch a repository into a projected working tree (NFS on macOS, FUSE on
Linux) after `[clone] enabled = true`:

```bash
bcode clone <url> [dir]
```

The default is a depth-1 checkout of the selected branch. Pass `--full-history`
for a complete clone. See [bcode clone](27-bcode-clone.md).

---

## First Launch

Start Bcode by running:

```bash
bcode
```

Bcode talks to whichever provider you configure, and to nothing else. There is
no bcode account to sign into -- your model provider is the account. Sign in
to one with:

```bash
bcode login
```

Pick a provider, paste its API key when prompted, and every model on that
provider works with nothing written to `config.toml`. Or skip the wizard and
export the key yourself:

```bash
export DEEPSEEK_API_KEY="sk-..."      # or whichever key your model names
bcode
```

Each model in the catalog names the variable its provider reads, so several
providers can be configured at once and `/model` switches between them.
Credentials Bcode stores itself live in `~/.bcode/`, and nothing is written
outside that directory, your workspace, and paths you name.

See [Authentication](02-authentication.md) for the full set of auth options
including named accounts, OIDC, and external auth providers.

---

## Basic Interaction

Once authenticated, Bcode presents a full-screen TUI with two main areas:

- **Scrollback** -- the conversation history showing your prompts, Bcode's responses, tool calls, file edits, and more.
- **Prompt** -- the input area at the bottom where you type messages.

Type a message and press `Enter` to send it. Bcode reads files, runs commands, and edits code as needed. Each tool run streams into the scrollback in real time.

Press `Tab` to move focus between the prompt and the scrollback. While a turn is running, `Esc` cancels it (the exception is fullscreen vim scrollback mode, where mid-turn `Esc` is a no-op; minimal mode cancels even with vim on); `Ctrl+C` cancels once the composer is empty — with a draft, the first press only clears it. Idle, press `Esc` twice within 800ms to clear a non-empty prompt, or (with an empty prompt and conversation messages) to open rewind — see [Keyboard Shortcuts](03-keyboard-shortcuts.md#escape). With the scrollback focused, use the arrow keys to select entries and to collapse or expand them. To navigate with `j`/`k` and fold with `h`/`l` instead, enable Vim mode.

### File References

Use `@` in your prompt to attach files:

```
@src/main.rs              # Attach a file
@src/main.rs:10-50        # Attach lines 10-50
@src/                     # Browse a directory
```

The `@` operator opens a fuzzy file picker. By default it respects `.gitignore` and hides dotfiles. Prefix with `!` to search hidden files:

```
@!.github                 # Search hidden files
@!.env                    # Attach a .env file
```

### Permissions

By default, Bcode asks for permission before executing shell commands or editing files. You can approve individually or toggle always-approve mode:

- Press `Ctrl+O` to toggle always-approve mode
- Use the `--yolo` flag at launch: `bcode --yolo`
- Type `/always-approve` in the prompt to toggle the mode

---

## Key Concepts

### Sessions

Every conversation is a **session**. Sessions are automatically saved to `~/.bcode/sessions/` and can be resumed later. Each session tracks the full conversation history, tool calls, file edits, and task state.

- Start a new session: `Ctrl+N` or `/new`
- Resume a previous session: `/resume` in the TUI, or `--resume <ID>` from the CLI
- Continue the most recent session: `bcode -c`

### Scrollback

The scrollback is the main display area. It shows:

- **User prompts** -- your messages, rendered as sticky headers
- **Agent messages** -- Bcode's responses with full markdown rendering and syntax highlighting
- **Thinking blocks** -- Bcode's reasoning process (collapsible)
- **Tool calls** -- file edits (with inline diffs), command executions, search results, and more
- **Task lists** -- TODO items tracking progress

Collapse or expand the selected entry with the `Left`/`Right` arrow keys (or `h`/`l` and `e` in Vim mode). In Vim mode, press `y` to copy its content and `Y` to copy its metadata (for example, the command that ran). Press `Enter` to open it in the fullscreen viewer (in any mode).

### Tools

Bcode has built-in tools for:

| Tool | Description |
|------|-------------|
| `read_file` / `search_replace` | Read and edit files with line-precise changes |
| `grep` | Regex search across your codebase (powered by ripgrep) |
| `list_dir` | List directory contents |
| `run_terminal_command` | Execute shell commands |
| `web_search` / `web_fetch` | Search the web and fetch URLs |
| `todo_write` | Create and manage task lists |
| `spawn_subagent` | Spawn parallel subagent sessions |
| `memory_search` | Search cross-session memory |

Tools can be extended with [MCP servers](05-configuration.md#mcp-servers) for integrations like GitHub, databases, and more.

### Slash Commands

Type `/` in the prompt to access commands. These provide quick actions without writing a full prompt:

```
/model deepseek-v4-pro                 # Switch model
/compact                          # Compress conversation history
/always-approve                   # Toggle always-approve mode
/new                              # Start a new session
```

See [Slash Commands](04-slash-commands.md) for the complete reference.

---

## Common Launch Options

```bash
# Launch the interactive TUI and submit an initial prompt as the first turn
bcode "fix the failing auth test and run it"

# Initial prompt in a new git worktree. Use --worktree=<name> (with `=`) so the
# prompt isn't swallowed as the worktree name — `bcode -w "refactor module X"`
# would treat "refactor module X" as the worktree label, not the prompt.
bcode --worktree=feat "refactor module X"

# Base the worktree on a specific branch (e.g. main) instead of the current HEAD:
bcode -w --ref main "implement feature from main"


# Start in a specific project directory
bcode --cwd ~/projects/my-app

# Add project-specific rules
bcode --rules "Always use TypeScript. Prefer functional components."

# Auto-approve all tool executions
bcode --yolo

# Use a specific model
bcode -m deepseek-v4-pro

# Resume a previous session
bcode --resume <session-id>

# Continue the most recent session
bcode -c

# Experimental scrollback-native render mode. Sticky: plain `bcode` reopens in
# the mode last chosen via --minimal/--fullscreen (or /minimal//fullscreen).
bcode --minimal

# Back to the standard fullscreen TUI (and make it sticky again)
bcode --fullscreen

# Headless mode (for scripts)
bcode -p "Explain this codebase"
```

---

## Headless Mode

Run Bcode non-interactively for scripting, CI/CD, and automation:

```bash
bcode -p "Your prompt here"
```

Output formats:

| Format | Flag | Description |
|--------|------|-------------|
| `plain` | (default) | Human-readable text |
| `json` | `--output-format json` | Single JSON object with `text`, `stopReason`, `sessionId`, and `requestId` |
| `streaming-json` | `--output-format streaming-json` | NDJSON event stream for real-time processing |

Example CI/CD usage:

```bash
bcode -p "Review changes for bugs" --output-format json --yolo | jq -r '.text'
```

---

## Project Rules (AGENTS.md)

Add per-project instructions by creating an `AGENTS.md` file in your repository. Bcode reads these files and injects their contents as a project-instructions message at the start of the conversation:

```
~/.bcode/AGENTS.md           # Global rules (apply to all projects)
<repo-root>/AGENTS.md       # Repository-level rules
<cwd>/AGENTS.md             # Directory-level rules (highest priority)
```

Deeper files take precedence. Bcode also reads `CLAUDE.md` files for compatibility.

---

## Where to Go Next

| Document | What You Will Learn |
|----------|-------------------|
| [Authentication](02-authentication.md) | Provider login, API keys, named accounts, OIDC, external auth |
| [Keyboard Shortcuts](03-keyboard-shortcuts.md) | Complete reference for all key bindings |
| [Slash Commands](04-slash-commands.md) | All available `/` commands |
| [Configuration](05-configuration.md) | config.toml, pager.toml, environment variables |
