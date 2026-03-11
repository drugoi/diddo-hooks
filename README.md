# diddo

`diddo` tracks your git commits and turns them into daily summaries.

It installs a global `post-commit` hook, stores commit metadata in a local SQLite database, and can summarize that history with an AI CLI tool or a direct API provider.

![diddo today output](assets/diddo.png)

## Install

Supported platforms: **macOS** (Apple Silicon, Intel), **Linux** (x86_64, aarch64), **Windows** (x86_64, ARM64). Pre-built binaries are published for each [release](https://github.com/drugoi/diddo-hooks/releases). Pre-built Linux binaries are built on Ubuntu 22.04 and require **glibc 2.35+** (e.g. Ubuntu 22.04 or Debian 12). On older distros, install from source: `cargo install --path .`.

### macOS and Linux

Install the latest release:

```bash
curl -sSL https://raw.githubusercontent.com/drugoi/diddo-hooks/main/install.sh | sh
```

To pin a version:

```bash
DIDDO_VERSION=0.1.0 curl -sSL https://raw.githubusercontent.com/drugoi/diddo-hooks/main/install.sh | sh
```

### Windows (PowerShell)

Install the latest release (run PowerShell as current user):

```powershell
irm https://raw.githubusercontent.com/drugoi/diddo-hooks/main/install.ps1 | iex
```

To pin a version:

```powershell
$env:DIDDO_VERSION = "0.1.0"; irm https://raw.githubusercontent.com/drugoi/diddo-hooks/main/install.ps1 | iex
```

Install options (environment variables):

- **DIDDO_VERSION** — Pin the install to a specific release (e.g. `0.1.0`).
- **DIDDO_INSTALL_DIR** — Directory where the binary is installed. Defaults: `$HOME/.local/bin` (macOS/Linux), `%LOCALAPPDATA%\diddo` (Windows).

Alternatively, download the `diddo-<version>-x86_64-pc-windows-msvc.zip` (or ARM64) from [Releases](https://github.com/drugoi/diddo-hooks/releases), extract `diddo.exe`, and add the folder to your PATH.

### From source (all platforms)

```bash
cargo install --path .
```

To try without installing:

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

On **Windows**, global hooks run only if you use **Git for Windows** (or another Git that runs hook scripts with a Unix-like shell). The generated hooks are `#!/bin/sh` scripts; Git for Windows runs them with its bundled sh.

`diddo` only records commits made after setup. It does not backfill old git history into the database.

To see where `diddo` stores its config, database, and managed hooks on your machine:

```bash
diddo config
```

Example output:

**macOS:**

```text
Config file: /Users/you/Library/Application Support/diddo/config.toml
Database path: /Users/you/Library/Application Support/diddo/commits.db
Hooks dir: /Users/you/Library/Application Support/diddo/hooks
```

**Linux:**

```text
Config file: /home/you/.config/diddo/config.toml
Database path: /home/you/.local/share/diddo/commits.db
Hooks dir: /home/you/.config/diddo/hooks
```

**Windows:**

```text
Config file: C:\Users\you\AppData\Roaming\diddo\config.toml
Database path: C:\Users\you\AppData\Local\diddo\commits.db
Hooks dir: C:\Users\you\AppData\Roaming\diddo\hooks
```

## Usage

Run `diddo` with no arguments in a terminal to launch **interactive mode** — an arrow-key menu of all available commands.

Show summaries:

```bash
diddo
diddo today
diddo yesterday
diddo week
diddo standup
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

- **`--md`** — Output summary as markdown.
- **`--json`** — Output summary as JSON.
- **`--raw`** — Skip AI and show grouped raw commit data.
- **`--no-cache`** — Skip the AI summary cache and force a fresh summary.

Current CLI behavior:

- `diddo standup` shows commits from the last 24 hours (`[now - 24h, now]`), useful when your daily meeting is in the afternoon
- `diddo` and `diddo today` are equivalent
- `--md`, `--json`, and `--raw` are summary-only flags
- `--raw` skips AI and shows grouped commit data
- `--md` and `--json` still try AI first unless you also use `--raw`
- If no commits are recorded for the selected period, `diddo` prints an empty-period message instead of failing

Summaries are grouped by git profile (`user.email`) then by repo; there is one AI summary per profile. Commits with no configured email are grouped under "unknown".

Other commands:

```bash
diddo init
diddo uninstall
diddo config
diddo metadata
```

- **`diddo metadata`** — Show database metadata: file size, total commit count, and oldest recorded commit.

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

- OpenAI: `gpt-4o-mini`
- Anthropic: `claude-sonnet-4-6`

### Default prompt

When `ai.prompt_instructions` is not set (or empty), the CLI uses a built-in prompt. There is no config key for the default; it is fixed in the binary. The prompt sent to the AI is (with `{period}` and commit count/list filled in):

```text
You are summarizing git activity for {period}.
Write a concise status update with the main themes, notable repos, and momentum.
Use only the commit data below.

Period: {period}
Commit count: {n}

Commits:
1. [repo_name] message (hash) on branch at 2026-03-10T12:00:00Z; files: 3, +12, -4
...

Return plain text only. Keep it brief and useful, in 2 short paragraphs max.
```

`{period}` is e.g. today, yesterday, or this week; `{n}` is the commit count. The list is one line per commit in the format above.

## Config File

The config file is optional. If it does not exist, `diddo` still works for raw summaries and for AI summaries when a supported CLI tool is already installed.

Run `diddo config` to get the exact config path on your machine.

Options:

| Key | Description |
|-----|-------------|
| `ai.provider` | API provider: `openai` or `anthropic` |
| `ai.api_key` | API key (overrides environment variables) |
| `ai.model` | Model for direct API; defaults: `gpt-4o-mini` (OpenAI), `claude-sonnet-4-6` (Anthropic) |
| `ai.prompt_instructions` | Custom AI instructions (tone, language, length); period and commit list are always appended |
| `ai.cli.prefer` | CLI/API preference: `api`, `cli`, `claude`, `codex`, `opencode`, or `cursor-agent` |

Example:

```toml
[ai]
provider = "openai"
model = "gpt-4o-mini"

[ai.cli]
prefer = "cli"
```

To use the direct API fallback, provide credentials either in the config file:

```toml
[ai]
provider = "anthropic"
api_key = "your-api-key"
model = "claude-sonnet-4-6"
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
- `ai.cli.prefer` accepts `api`, `cli`, `claude`, `codex`, `opencode`, or `cursor-agent` (alias: `cursor_agent`)
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
