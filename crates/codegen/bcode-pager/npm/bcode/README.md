# Bcode

Bring Bcode into your terminal. Fast, flicker-free CLI built for plans, subagents, and parallel work.

**[Homepage](https://bcode.invalid/cli)** | **[Documentation](https://docs.invalid/build/overview)**

## Install

```bash
curl -fsSL https://bcode.invalid/cli/install.sh | bash
```

Or install with npm:

```bash
npm i -g @bcode-official/bcode
```

## Get Started

```bash
# Launch the interactive TUI
bcode

# Run a single task
bcode -p "Explain this codebase"
```

On first launch, Bcode opens your browser to authenticate. For CI or headless environments, use an API key from [console.invalid](https://console.invalid):

```bash
export BCODE_API_KEY="bcode-..."
```

## Update

```bash
bcode update
```

Or if installed via npm:

```bash
npm i -g @bcode-official/bcode@latest
```

## Supported Platforms

| Platform | Architecture |
|---|---|
| macOS | Apple Silicon (arm64) |
| Linux | x86_64, arm64 |
| Windows | x86_64 |

## Documentation

For full documentation including configuration, MCP servers, custom models, headless mode, agent mode, and more, visit [docs.invalid/build/overview](https://docs.invalid/build/overview).

## Feedback

Run `/feedback` inside Bcode to report issues or send feedback directly.
