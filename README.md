# diddo

`diddo` tracks your git commits and turns them into daily summaries.

It installs a global `post-commit` hook, stores commit metadata in a local SQLite database, and can summarize that history with an AI CLI tool or a direct API provider.

## Install

Install the latest release (macOS; requires a [release](https://github.com/drugoi/diddo-hooks/releases) to exist):

```bash
curl -sSL https://raw.githubusercontent.com/drugoi/diddo-hooks/main/install.sh | sh
```

To pin a version:

```bash
DIDDO_VERSION=0.1.0 curl -sSL https://raw.githubusercontent.com/drugoi/diddo-hooks/main/install.sh | sh
```

Or build and install from source:

```bash
cargo install --path .
```

To try it without installing:

```bash
cargo run -- --help
```

## Setup

Install the managed global hooks directory:

```bash
diddo init
```

What `diddo init` does:

- Creates a managed hooks directory for `diddo`
- Sets global git `core.hooksPath` to that directory
- Preserves and forwards any previously configured global hooks so existing hooks keep running

`diddo` only records commits made after setup. It does not backfill old git history into the database.

To see where `diddo` stores its config, database, and managed hooks on your machine:

```bash
diddo config
```

On macOS, that currently looks like:

```text
Config file: /Users/you/Library/Application Support/diddo/config.toml
Database path: /Users/you/Library/Application Support/diddo/commits.db
Hooks dir: /Users/you/Library/Application Support/diddo/hooks
```

## Usage

Show summaries:

```bash
diddo
diddo today
diddo yesterday
diddo week
```

Output modes:

```bash
diddo --md
diddo --json
diddo --raw
diddo --no-cache

diddo today --md
diddo yesterday --json
diddo week --raw
diddo today --no-cache
```

- **`--no-cache`** — Skip the AI summary cache and force a fresh summary.

Current CLI behavior:

- `diddo` and `diddo today` are equivalent
- `--md`, `--json`, and `--raw` are summary-only flags
- `--raw` skips AI and shows grouped commit data
- `--md` and `--json` still try AI first unless you also use `--raw`
- If no commits are recorded for the selected period, `diddo` prints an empty-period message instead of failing

Other commands:

```bash
diddo init
diddo uninstall
diddo config
```

## AI Providers

`diddo` is CLI-first by default.

AI summaries are **cached** in the same SQLite database as your commits. When the commit set and period are unchanged (and you use the same provider/model), `diddo` returns the stored summary instead of calling the AI again. Use `--no-cache` to force a fresh summary.

Without extra configuration, it tries installed AI CLIs in this order:

1. `claude`
2. `codex`
3. `opencode`
4. `cursor`

If no supported CLI is available, `diddo` falls back to a direct API provider when configuration and credentials are available.

If neither CLI tools nor a usable API configuration are available, `diddo` falls back to grouped raw commit output and prints a warning on stderr.

### Provider selection rules

- Default behavior: try detected CLI tools first, then try API
- `ai.cli.prefer = "cli"`: keep CLI-first behavior, then API fallback if configured
- `ai.cli.prefer = "claude"`, `"codex"`, `"opencode"`, or `"cursor-agent"`: force that CLI first, then API fallback if configured
- `ai.cli.prefer = "api"`: skip CLI detection and use API only

Supported API providers:

- `openai`
- `anthropic`

Default API models:

- OpenAI: `gpt-4.1-mini`
- Anthropic: `claude-3-7-sonnet-latest`

## Config File

The config file is optional. If it does not exist, `diddo` still works for raw summaries and for AI summaries when a supported CLI tool is already installed.

Run `diddo config` to get the exact config path on your machine.

Example:

```toml
[ai]
provider = "openai"
model = "gpt-4.1-mini"

[ai.cli]
prefer = "cli"
```

To use the direct API fallback, provide credentials either in the config file:

```toml
[ai]
provider = "anthropic"
api_key = "your-api-key"
model = "claude-3-7-sonnet-latest"
prompt_instructions = "Summarize in German. One short paragraph, plain text only."

[ai.cli]
prefer = "cli"
```

or through environment variables:

```bash
export DIDDO_OPENAI_KEY=your-openai-key
export DIDDO_ANTHROPIC_KEY=your-anthropic-key
```

Notes:

- `ai.provider` accepts `openai` or `anthropic`
- `ai.cli.prefer` accepts `api`, `cli`, `claude`, `codex`, `opencode`, or `cursor-agent`
- `ai.model` applies to direct API providers only; CLI tools use their own default model selection
- `ai.api_key` in the config file overrides environment variables
- `ai.prompt_instructions` (optional): when set, replaces the default AI instructions (tone, language, length); period, commit count, and commit list are always appended. Use for a different language or more concise output. Empty or missing = default instructions.
- If `ai.provider` is omitted and exactly one of `DIDDO_OPENAI_KEY` or `DIDDO_ANTHROPIC_KEY` is set, `diddo` infers the provider from that environment variable

## Uninstall

Remove `diddo` from your global git hooks setup:

```bash
diddo uninstall
```

Current uninstall behavior:

- Removes the managed `diddo` hooks directory
- Restores the previous global `core.hooksPath` if `diddo` still owns that setting
- Unsets global `core.hooksPath` if `diddo` created it and there was no previous value
- Leaves the current global `core.hooksPath` untouched if you changed it after installing `diddo`

## License

MIT
