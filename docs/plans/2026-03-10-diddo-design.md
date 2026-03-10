# diddo — Design Document

A cross-platform CLI tool that captures git commit data via a global post-commit hook, stores it locally, and generates AI-powered daily work summaries.

## CLI Interface

```
diddo                     # summary for today
diddo yesterday           # summary for yesterday
diddo week                # summary for current week
diddo init                # install global post-commit hook
diddo uninstall           # remove global hook, clean up
diddo hook                # (internal) called by post-commit hook
diddo config              # open/print config location
```

Flags on summary commands:

```
--md                      # output as markdown
--raw                     # skip AI, show grouped raw commits
--json                    # output as JSON (for scripting)
```

## Stack

- **Language:** Rust
- **Database:** SQLite via `rusqlite`
- **CLI parsing:** `clap`
- **HTTP:** `reqwest` (for direct API fallback)
- **Terminal output:** `colored` or `crossterm` for styling

## Architecture

Single binary, two modes:

1. **Hook mode** (`diddo hook`) — called by the post-commit hook. Extracts commit metadata and writes to SQLite. Must complete in <100ms.
2. **Summary mode** (`diddo`, `diddo week`, etc.) — queries DB, sends data to AI, renders output.

## Data Model

SQLite database location:
- Linux/macOS: `~/.local/share/diddo/commits.db`
- Windows: `%LOCALAPPDATA%\diddo\commits.db`

### `commits` table

| Column         | Type       | Description                        |
|----------------|------------|------------------------------------|
| `id`           | INTEGER PK | autoincrement                      |
| `hash`         | TEXT       | short commit hash                  |
| `message`      | TEXT       | full commit message                |
| `repo_path`    | TEXT       | absolute path to the repo          |
| `repo_name`    | TEXT       | directory name (e.g. "diddo")      |
| `branch`       | TEXT       | branch name at commit time         |
| `files_changed`| INTEGER    | number of files changed            |
| `insertions`   | INTEGER    | lines added                        |
| `deletions`    | INTEGER    | lines removed                      |
| `committed_at` | TEXT       | ISO 8601 timestamp                 |

Index: composite on `(committed_at, repo_name)`.

### Data captured by hook

All from git commands, no external dependencies:

- `git rev-parse --short HEAD` — hash
- `git log -1 --format=%B` — message
- `git rev-parse --show-toplevel` — repo path
- `git rev-parse --abbrev-ref HEAD` — branch
- `git diff --stat HEAD~1..HEAD` — diff stats

## AI Provider Layer

### Priority chain (first available wins)

1. **CLI tools** (zero config) — detect installed tools in PATH:
   - `claude` (Claude Code CLI) — `claude -p "..."`
   - `codex` (OpenAI Codex CLI)
   - Detection via `which` / `where`

2. **Direct API** (requires config):
   - OpenAI API (`DIDDO_OPENAI_KEY` env var or config file)
   - Anthropic API (`DIDDO_ANTHROPIC_KEY` env var or config file)
   - HTTP via `reqwest`, no heavy SDK dependencies

3. **No AI available** — graceful degradation:
   - Print grouped raw commit list (same as `--raw`)
   - Hint: "Install claude or set an API key for AI summaries"

### Config file

Location: `~/.config/diddo/config.toml` (`%APPDATA%\diddo\config.toml` on Windows)

```toml
[ai]
provider = "openai"           # "openai" | "anthropic"
api_key = "sk-..."            # or use DIDDO_OPENAI_KEY env var
model = "gpt-4o-mini"         # sensible cheap default

[ai.cli]
prefer = "claude"             # force a specific CLI tool
```

### AI prompt

Hardcoded, not user-configurable. The prompt instructs the AI to:

- Summarize commits grouped by project
- Cluster related commits by meaning under short topic headings
- Keep summaries concise
- Return structured data (project → topic groups → commit hashes)

## Hook Installation

### `diddo init`

1. Creates `~/.config/diddo/hooks/` directory
2. Writes a `post-commit` script: `#!/bin/sh\ndiddo hook`
3. Runs `git config --global core.hooksPath ~/.config/diddo/hooks/`
4. If `core.hooksPath` was already set, chains existing hooks — copies existing post-commit to `post-commit.prev`, new script calls both
5. Prints confirmation

### `diddo uninstall`

Removes `core.hooksPath` global config and cleans up hook directory.

## Output Format

### Terminal (default)

```
  Tuesday, Mar 10, 2026

  diddo (4 commits)
    CLI & argument parsing
      a1b2c3f  Add clap dependency and basic CLI skeleton
      d4e5f6a  Implement subcommands for today/week/yesterday
    Data layer
      b7c8d9e  Create SQLite schema and migrations
      e0f1a2b  Add commit insertion and date-range queries

  my-website (2 commits)
    Bug fixes
      c3d4e5f  Fix responsive grid on pricing page
      f6a7b8c  Correct media query breakpoint for mobile

  ───────────────────────
  6 commits across 2 projects
  First commit: 09:12 · Last: 17:45
  Most active: diddo (4 commits)
```

Structure:
- Grouped by project, sorted by commit count descending
- Within each project, AI clusters related commits under short topic headings
- Each commit shows short hash + original message
- Stats footer: total commits, project count, time span, most active project

### Markdown (`--md`)

Same structure using `##` for projects, `###` for topic groups, `-` for commit lines.

### JSON (`--json`)

Structured object with projects array, each containing topic groups with commits. For scripting and integration.

## Key Design Decisions

- **One table, no caching** — commit volume is small (20-50/day), AI is called only on explicit request
- **Repo name from path** — derived from directory name, no separate projects table
- **Hook chains existing hooks** — doesn't break per-repo or previously-configured global hooks
- **CLI tools preferred over API keys** — zero-config path leverages tools users already pay for
- **Graceful degradation** — works without AI, just shows raw grouped commits
