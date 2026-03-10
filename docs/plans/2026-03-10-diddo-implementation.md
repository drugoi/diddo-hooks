# diddo Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a cross-platform Rust CLI tool that captures git commit data via a global post-commit hook and generates AI-powered daily work summaries.

**Architecture:** Single binary with two modes — hook mode (fast SQLite write on every commit) and summary mode (query + AI + render). CLI tools like `claude`/`codex` are preferred AI providers, with direct API fallback. Graceful degradation to raw output when no AI is available.

**Tech Stack:** Rust, SQLite (`rusqlite`), `clap` (CLI), `reqwest` (HTTP), `directories` (XDG paths), `chrono` (dates), `crossterm` (terminal styling), `serde`/`toml` (config), `tokio` (async runtime for reqwest)

---

### Task 1: Project Scaffolding

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `.gitignore`

**Step 1: Initialize Cargo project**

Run: `cargo init --name diddo`

**Step 2: Add dependencies to Cargo.toml**

```toml
[package]
name = "diddo"
version = "0.1.0"
edition = "2021"
description = "Track your git commits, get AI-powered daily summaries"
license = "MIT"

[dependencies]
clap = { version = "4", features = ["derive"] }
rusqlite = { version = "0.32", features = ["bundled"] }
directories = "5"
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
tokio = { version = "1", features = ["rt", "macros"] }
crossterm = "0.28"
anyhow = "1"
```

Note: We use `rusqlite` with `bundled` feature so SQLite is compiled in — no system dependency needed for cross-platform builds.

**Step 3: Set up basic main.rs with clap skeleton**

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "diddo", about = "Track your git commits, get AI-powered daily summaries")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Output as markdown
    #[arg(long)]
    md: bool,

    /// Skip AI, show raw grouped commits
    #[arg(long)]
    raw: bool,

    /// Output as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Show yesterday's summary
    Yesterday,
    /// Show this week's summary
    Week,
    /// Install global post-commit hook
    Init,
    /// Remove global hook and clean up
    Uninstall,
    /// (internal) Called by post-commit hook
    Hook,
    /// Show config file location
    Config,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => println!("TODO: show today's summary"),
        Some(Commands::Yesterday) => println!("TODO: yesterday"),
        Some(Commands::Week) => println!("TODO: week"),
        Some(Commands::Init) => println!("TODO: init"),
        Some(Commands::Uninstall) => println!("TODO: uninstall"),
        Some(Commands::Hook) => println!("TODO: hook"),
        Some(Commands::Config) => println!("TODO: config"),
    }

    Ok(())
}
```

**Step 4: Verify it compiles and runs**

Run: `cargo build`
Expected: Compiles with no errors.

Run: `cargo run`
Expected: Prints "TODO: show today's summary"

Run: `cargo run -- --help`
Expected: Shows help with all subcommands listed.

**Step 5: Add .gitignore**

```
/target
```

**Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs .gitignore
git commit -m "feat: scaffold diddo project with clap CLI skeleton"
```

---

### Task 2: Database Module — Schema & Connection

**Files:**
- Create: `src/db.rs`
- Modify: `src/main.rs` (add `mod db;`)

**Step 1: Write the test**

Add to `src/db.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_creates_table_on_init() {
        let db = Database::open_in_memory().unwrap();
        let count: i64 = db.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='commits'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 1);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -- test_creates_table_on_init`
Expected: FAIL — `Database` not defined.

**Step 3: Implement Database struct with schema creation**

```rust
use anyhow::Result;
use rusqlite::Connection;

pub struct Database {
    pub conn: Connection,
}

impl Database {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS commits (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                hash          TEXT NOT NULL,
                message       TEXT NOT NULL,
                repo_path     TEXT NOT NULL,
                repo_name     TEXT NOT NULL,
                branch        TEXT NOT NULL,
                files_changed INTEGER NOT NULL DEFAULT 0,
                insertions    INTEGER NOT NULL DEFAULT 0,
                deletions     INTEGER NOT NULL DEFAULT 0,
                committed_at  TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_commits_date_repo
                ON commits (committed_at, repo_name);"
        )?;
        Ok(())
    }
}
```

**Step 4: Add `mod db;` to main.rs**

Add at the top of `src/main.rs`:

```rust
mod db;
```

**Step 5: Run test to verify it passes**

Run: `cargo test -- test_creates_table_on_init`
Expected: PASS

**Step 6: Commit**

```bash
git add src/db.rs src/main.rs
git commit -m "feat: add database module with SQLite schema and migrations"
```

---

### Task 3: Database Module — Insert & Query

**Files:**
- Modify: `src/db.rs`

**Step 1: Define the Commit struct**

Add to top of `src/db.rs`:

```rust
use chrono::{DateTime, Utc, NaiveDate};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Commit {
    pub id: Option<i64>,
    pub hash: String,
    pub message: String,
    pub repo_path: String,
    pub repo_name: String,
    pub branch: String,
    pub files_changed: i64,
    pub insertions: i64,
    pub deletions: i64,
    pub committed_at: DateTime<Utc>,
}
```

**Step 2: Write tests for insert and query**

```rust
#[test]
fn test_insert_and_query_today() {
    let db = Database::open_in_memory().unwrap();
    let commit = Commit {
        id: None,
        hash: "abc1234".into(),
        message: "fix: resolve login bug".into(),
        repo_path: "/home/user/projects/my-app".into(),
        repo_name: "my-app".into(),
        branch: "main".into(),
        files_changed: 3,
        insertions: 25,
        deletions: 10,
        committed_at: Utc::now(),
    };
    db.insert_commit(&commit).unwrap();

    let today = Utc::now().date_naive();
    let commits = db.query_date(today).unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].hash, "abc1234");
    assert_eq!(commits[0].repo_name, "my-app");
}

#[test]
fn test_query_date_range() {
    let db = Database::open_in_memory().unwrap();
    let now = Utc::now();
    let yesterday = now - chrono::Duration::days(1);

    let commit_today = Commit {
        id: None,
        hash: "aaa1111".into(),
        message: "today's commit".into(),
        repo_path: "/home/user/proj".into(),
        repo_name: "proj".into(),
        branch: "main".into(),
        files_changed: 1,
        insertions: 5,
        deletions: 0,
        committed_at: now,
    };
    let commit_yesterday = Commit {
        id: None,
        hash: "bbb2222".into(),
        message: "yesterday's commit".into(),
        repo_path: "/home/user/proj".into(),
        repo_name: "proj".into(),
        branch: "main".into(),
        files_changed: 2,
        insertions: 10,
        deletions: 3,
        committed_at: yesterday,
    };
    db.insert_commit(&commit_today).unwrap();
    db.insert_commit(&commit_yesterday).unwrap();

    let today_date = now.date_naive();
    let results = db.query_date(today_date).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].hash, "aaa1111");

    let yesterday_date = yesterday.date_naive();
    let week_results = db.query_date_range(yesterday_date, today_date).unwrap();
    assert_eq!(week_results.len(), 2);
}
```

**Step 3: Run tests to verify they fail**

Run: `cargo test`
Expected: FAIL — `insert_commit`, `query_date`, `query_date_range` not defined.

**Step 4: Implement insert and query methods**

Add to `impl Database`:

```rust
pub fn insert_commit(&self, commit: &Commit) -> Result<()> {
    self.conn.execute(
        "INSERT INTO commits (hash, message, repo_path, repo_name, branch, files_changed, insertions, deletions, committed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            commit.hash,
            commit.message,
            commit.repo_path,
            commit.repo_name,
            commit.branch,
            commit.files_changed,
            commit.insertions,
            commit.deletions,
            commit.committed_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn query_date(&self, date: NaiveDate) -> Result<Vec<Commit>> {
    let start = format!("{}T00:00:00+00:00", date);
    let end = format!("{}T00:00:00+00:00", date.succ_opt().unwrap());
    self.query_date_range_raw(&start, &end)
}

pub fn query_date_range(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<Commit>> {
    let start = format!("{}T00:00:00+00:00", from);
    let end = format!("{}T00:00:00+00:00", to.succ_opt().unwrap());
    self.query_date_range_raw(&start, &end)
}

fn query_date_range_raw(&self, start: &str, end: &str) -> Result<Vec<Commit>> {
    let mut stmt = self.conn.prepare(
        "SELECT id, hash, message, repo_path, repo_name, branch, files_changed, insertions, deletions, committed_at
         FROM commits
         WHERE committed_at >= ?1 AND committed_at < ?2
         ORDER BY repo_name, committed_at"
    )?;
    let commits = stmt.query_map(rusqlite::params![start, end], |row| {
        let committed_at_str: String = row.get(9)?;
        let committed_at = DateTime::parse_from_rfc3339(&committed_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        Ok(Commit {
            id: Some(row.get(0)?),
            hash: row.get(1)?,
            message: row.get(2)?,
            repo_path: row.get(3)?,
            repo_name: row.get(4)?,
            branch: row.get(5)?,
            files_changed: row.get(6)?,
            insertions: row.get(7)?,
            deletions: row.get(8)?,
            committed_at,
        })
    })?.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(commits)
}
```

**Step 5: Run tests to verify they pass**

Run: `cargo test`
Expected: All tests PASS.

**Step 6: Commit**

```bash
git add src/db.rs
git commit -m "feat: add commit insert and date-range query methods"
```

---

### Task 4: Paths Module — Cross-Platform Directories

**Files:**
- Create: `src/paths.rs`
- Modify: `src/main.rs` (add `mod paths;`)

**Step 1: Implement paths module**

```rust
use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::path::PathBuf;

pub struct AppPaths {
    pub db_path: PathBuf,
    pub config_path: PathBuf,
    pub hooks_dir: PathBuf,
}

impl AppPaths {
    pub fn new() -> Result<Self> {
        let proj = ProjectDirs::from("", "", "diddo")
            .context("Could not determine home directory")?;

        let data_dir = proj.data_dir().to_path_buf();
        let config_dir = proj.config_dir().to_path_buf();

        Ok(Self {
            db_path: data_dir.join("commits.db"),
            config_path: config_dir.join("config.toml"),
            hooks_dir: config_dir.join("hooks"),
        })
    }
}
```

**Step 2: Add `mod paths;` to main.rs**

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles.

**Step 4: Commit**

```bash
git add src/paths.rs src/main.rs
git commit -m "feat: add cross-platform paths module using directories crate"
```

---

### Task 5: Hook Mode — Git Data Extraction

**Files:**
- Create: `src/hook.rs`
- Modify: `src/main.rs` (add `mod hook;`)

**Step 1: Implement hook module that extracts git data and stores it**

```rust
use anyhow::{Context, Result};
use chrono::Utc;
use std::process::Command;

use crate::db::{Commit, Database};

pub fn run(db: &Database) -> Result<()> {
    let hash = git_cmd(&["rev-parse", "--short", "HEAD"])?;
    let message = git_cmd(&["log", "-1", "--format=%B"])?;
    let repo_path = git_cmd(&["rev-parse", "--show-toplevel"])?;
    let branch = git_cmd(&["rev-parse", "--abbrev-ref", "HEAD"])?;

    let repo_name = std::path::Path::new(&repo_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".into());

    let (files_changed, insertions, deletions) = parse_diff_stats();

    let commit = Commit {
        id: None,
        hash: hash.trim().to_string(),
        message: message.trim().to_string(),
        repo_path: repo_path.trim().to_string(),
        repo_name,
        branch: branch.trim().to_string(),
        files_changed,
        insertions,
        deletions,
        committed_at: Utc::now(),
    };

    db.insert_commit(&commit)?;
    Ok(())
}

fn git_cmd(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .context("Failed to run git")?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn parse_diff_stats() -> (i64, i64, i64) {
    let output = Command::new("git")
        .args(["diff", "--stat", "HEAD~1..HEAD"])
        .output();

    let Ok(output) = output else {
        return (0, 0, 0);
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let last_line = text.lines().last().unwrap_or("");

    let mut files = 0i64;
    let mut ins = 0i64;
    let mut del = 0i64;

    for part in last_line.split(',') {
        let part = part.trim();
        if part.contains("file") {
            files = part.split_whitespace().next()
                .and_then(|n| n.parse().ok()).unwrap_or(0);
        } else if part.contains("insertion") {
            ins = part.split_whitespace().next()
                .and_then(|n| n.parse().ok()).unwrap_or(0);
        } else if part.contains("deletion") {
            del = part.split_whitespace().next()
                .and_then(|n| n.parse().ok()).unwrap_or(0);
        }
    }

    (files, ins, del)
}
```

**Step 2: Wire hook command in main.rs**

In `main.rs`, update the `Hook` arm:

```rust
Some(Commands::Hook) => {
    let paths = paths::AppPaths::new()?;
    let db = db::Database::open(&paths.db_path)?;
    hook::run(&db)?;
}
```

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles.

**Step 4: Test manually in this repo**

Run: `cargo run -- hook`
Expected: No errors (inserts current repo's latest commit into DB).

**Step 5: Commit**

```bash
git add src/hook.rs src/main.rs
git commit -m "feat: implement hook mode — extract git data and store in SQLite"
```

---

### Task 6: Init & Uninstall Commands

**Files:**
- Create: `src/init.rs`
- Modify: `src/main.rs` (add `mod init;`)

**Step 1: Implement init module**

```rust
use anyhow::{Context, Result};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use crate::paths::AppPaths;

pub fn install(paths: &AppPaths) -> Result<()> {
    fs::create_dir_all(&paths.hooks_dir)?;

    let existing_hooks_path = get_existing_hooks_path();

    if let Some(ref existing) = existing_hooks_path {
        let existing_hook = std::path::Path::new(existing).join("post-commit");
        if existing_hook.exists() {
            let prev = paths.hooks_dir.join("post-commit.prev");
            fs::copy(&existing_hook, &prev)
                .context("Failed to backup existing post-commit hook")?;
            println!("Backed up existing post-commit hook to {}", prev.display());
        }
    }

    let hook_script = build_hook_script(paths);
    let hook_path = paths.hooks_dir.join("post-commit");
    fs::write(&hook_path, hook_script)?;

    #[cfg(unix)]
    fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))?;

    Command::new("git")
        .args(["config", "--global", "core.hooksPath", &paths.hooks_dir.to_string_lossy()])
        .output()
        .context("Failed to set global core.hooksPath")?;

    println!("Global post-commit hook installed.");
    println!("Hooks directory: {}", paths.hooks_dir.display());
    println!("Your commits are now being tracked by diddo.");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    Command::new("git")
        .args(["config", "--global", "--unset", "core.hooksPath"])
        .output()
        .context("Failed to unset core.hooksPath")?;

    println!("Global post-commit hook removed.");
    println!("Your commits are no longer being tracked.");
    Ok(())
}

fn get_existing_hooks_path() -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--global", "--get", "core.hooksPath"])
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() { None } else { Some(path) }
}

fn build_hook_script(paths: &AppPaths) -> String {
    let prev = paths.hooks_dir.join("post-commit.prev");
    let chain = if prev.exists() {
        format!("\n# Chain previously existing hook\nif [ -x \"{}\" ]; then\n  \"{}\"\nfi\n", prev.display(), prev.display())
    } else {
        String::new()
    };

    format!(
        "#!/bin/sh\ndiddo hook\n{}", chain
    )
}
```

**Step 2: Wire in main.rs**

```rust
Some(Commands::Init) => {
    let paths = paths::AppPaths::new()?;
    init::install(&paths)?;
}
Some(Commands::Uninstall) => {
    init::uninstall()?;
}
```

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles.

**Step 4: Commit**

```bash
git add src/init.rs src/main.rs
git commit -m "feat: implement init and uninstall commands for global hook management"
```

---

### Task 7: Config Module

**Files:**
- Create: `src/config.rs`
- Modify: `src/main.rs` (add `mod config;`)

**Step 1: Implement config parsing**

```rust
use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub ai: AiConfig,
}

#[derive(Debug, Deserialize, Default)]
pub struct AiConfig {
    pub provider: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub cli: CliConfig,
}

#[derive(Debug, Deserialize, Default)]
pub struct CliConfig {
    pub prefer: Option<String>,
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        let config: AppConfig = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn api_key(&self) -> Option<String> {
        self.ai.api_key.clone()
            .or_else(|| std::env::var("DIDDO_OPENAI_KEY").ok())
            .or_else(|| std::env::var("DIDDO_ANTHROPIC_KEY").ok())
    }
}
```

**Step 2: Wire config command in main.rs**

```rust
Some(Commands::Config) => {
    let paths = paths::AppPaths::new()?;
    println!("Config file: {}", paths.config_path.display());
    println!("Database:    {}", paths.db_path.display());
    println!("Hooks dir:   {}", paths.hooks_dir.display());
}
```

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles.

**Step 4: Commit**

```bash
git add src/config.rs src/main.rs
git commit -m "feat: add config module with TOML parsing and env var fallback"
```

---

### Task 8: AI Provider Layer

**Files:**
- Create: `src/ai/mod.rs`
- Create: `src/ai/cli_provider.rs`
- Create: `src/ai/api_provider.rs`
- Modify: `src/main.rs` (add `mod ai;`)

**Step 1: Define the provider trait and prompt builder**

`src/ai/mod.rs`:

```rust
pub mod cli_provider;
pub mod api_provider;

use anyhow::Result;
use crate::db::Commit;
use crate::config::AppConfig;

pub trait AiProvider {
    fn summarize(&self, commits: &[Commit], period: &str) -> Result<String>;
}

pub fn get_provider(config: &AppConfig) -> Option<Box<dyn AiProvider>> {
    if let Some(ref prefer) = config.ai.cli.prefer {
        if let Some(p) = cli_provider::CliProvider::new(prefer) {
            return Some(Box::new(p));
        }
    }

    for tool in &["claude", "codex"] {
        if let Some(p) = cli_provider::CliProvider::new(tool) {
            return Some(Box::new(p));
        }
    }

    if let Some(key) = config.api_key() {
        let provider = config.ai.provider.as_deref().unwrap_or("openai");
        let model = config.ai.model.as_deref().unwrap_or("gpt-4o-mini");
        return Some(Box::new(api_provider::ApiProvider::new(
            provider.to_string(),
            key,
            model.to_string(),
        )));
    }

    None
}

pub fn build_prompt(commits: &[Commit], period: &str) -> String {
    let mut prompt = format!(
        "Here are my git commits from {}. Summarize what I worked on.\n\n\
         Rules:\n\
         - Group by project (repo name)\n\
         - Within each project, cluster related commits under a short topic heading\n\
         - Under each topic, list the individual commits with their short hash and original message\n\
         - Keep topic headings concise (2-4 words)\n\
         - At the end, provide stats: total commits, number of projects, time span\n\n\
         Format exactly like this:\n\
         PROJECT_NAME (N commits)\n\
           Topic heading\n\
             HASH  original commit message\n\
             HASH  original commit message\n\
           Another topic\n\
             HASH  original commit message\n\n\
         Commits:\n\n",
        period
    );

    for c in commits {
        prompt.push_str(&format!(
            "repo:{} hash:{} branch:{} files:{} +{} -{}\n  {}\n\n",
            c.repo_name, c.hash, c.branch, c.files_changed, c.insertions, c.deletions, c.message
        ));
    }

    prompt
}
```

**Step 2: Implement CLI provider**

`src/ai/cli_provider.rs`:

```rust
use anyhow::{Context, Result};
use std::process::Command;

use super::{AiProvider, build_prompt};
use crate::db::Commit;

pub struct CliProvider {
    tool: String,
}

impl CliProvider {
    pub fn new(tool: &str) -> Option<Self> {
        let check = if cfg!(windows) { "where" } else { "which" };
        let found = Command::new(check).arg(tool).output().ok()
            .map(|o| o.status.success()).unwrap_or(false);
        if found { Some(Self { tool: tool.to_string() }) } else { None }
    }
}

impl AiProvider for CliProvider {
    fn summarize(&self, commits: &[Commit], period: &str) -> Result<String> {
        let prompt = build_prompt(commits, period);

        let output = match self.tool.as_str() {
            "claude" => Command::new("claude")
                .args(["-p", &prompt])
                .output()
                .context("Failed to run claude CLI")?,
            "codex" => Command::new("codex")
                .args(["-q", &prompt])
                .output()
                .context("Failed to run codex CLI")?,
            other => Command::new(other)
                .arg(&prompt)
                .output()
                .context(format!("Failed to run {} CLI", other))?,
        };

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}
```

**Step 3: Implement API provider**

`src/ai/api_provider.rs`:

```rust
use anyhow::{Context, Result};

use super::{AiProvider, build_prompt};
use crate::db::Commit;

pub struct ApiProvider {
    provider: String,
    api_key: String,
    model: String,
}

impl ApiProvider {
    pub fn new(provider: String, api_key: String, model: String) -> Self {
        Self { provider, api_key, model }
    }

    fn endpoint(&self) -> &str {
        match self.provider.as_str() {
            "anthropic" => "https://api.anthropic.com/v1/messages",
            _ => "https://api.openai.com/v1/chat/completions",
        }
    }
}

impl AiProvider for ApiProvider {
    fn summarize(&self, commits: &[Commit], period: &str) -> Result<String> {
        let prompt = build_prompt(commits, period);
        let rt = tokio::runtime::Runtime::new()?;

        rt.block_on(async {
            let client = reqwest::Client::new();

            let response = match self.provider.as_str() {
                "anthropic" => {
                    let body = serde_json::json!({
                        "model": self.model,
                        "max_tokens": 2048,
                        "messages": [{"role": "user", "content": prompt}]
                    });
                    client.post(self.endpoint())
                        .header("x-api-key", &self.api_key)
                        .header("anthropic-version", "2023-06-01")
                        .json(&body)
                        .send().await
                        .context("Anthropic API request failed")?
                }
                _ => {
                    let body = serde_json::json!({
                        "model": self.model,
                        "messages": [{"role": "user", "content": prompt}]
                    });
                    client.post(self.endpoint())
                        .header("Authorization", format!("Bearer {}", self.api_key))
                        .json(&body)
                        .send().await
                        .context("OpenAI API request failed")?
                }
            };

            let json: serde_json::Value = response.json().await?;

            let text = match self.provider.as_str() {
                "anthropic" => json["content"][0]["text"].as_str().unwrap_or("").to_string(),
                _ => json["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string(),
            };

            Ok(text)
        })
    }
}
```

**Step 4: Add `mod ai;` to main.rs**

**Step 5: Verify it compiles**

Run: `cargo build`
Expected: Compiles.

**Step 6: Commit**

```bash
git add src/ai/ src/main.rs
git commit -m "feat: implement AI provider layer with CLI detection and API fallback"
```

---

### Task 9: Output Rendering

**Files:**
- Create: `src/render.rs`
- Modify: `src/main.rs` (add `mod render;`)

**Step 1: Implement terminal and markdown renderers**

```rust
use crate::db::Commit;
use crossterm::style::{Stylize, Attribute};

pub struct SummaryData {
    pub date_label: String,
    pub ai_summary: Option<String>,
    pub commits: Vec<Commit>,
    pub total_commits: usize,
    pub project_count: usize,
    pub first_commit_time: String,
    pub last_commit_time: String,
    pub most_active_project: String,
    pub most_active_count: usize,
}

pub fn render_terminal(data: &SummaryData) {
    println!();
    println!("  {}", data.date_label.clone().bold());
    println!();

    if let Some(ref summary) = data.ai_summary {
        for line in summary.lines() {
            println!("  {}", line);
        }
    } else {
        render_raw_terminal(&data.commits);
    }

    println!();
    println!("  {}", "───────────────────────".dark_grey());
    println!(
        "  {} commits across {} projects",
        data.total_commits, data.project_count
    );
    println!(
        "  First commit: {} · Last: {}",
        data.first_commit_time, data.last_commit_time
    );
    println!(
        "  Most active: {} ({} commits)",
        data.most_active_project, data.most_active_count
    );
    println!();
}

pub fn render_raw_terminal(commits: &[Commit]) {
    let mut current_repo = String::new();
    let mut repo_count = 0;

    for c in commits {
        if c.repo_name != current_repo {
            if !current_repo.is_empty() {
                println!();
            }
            repo_count = commits.iter().filter(|x| x.repo_name == c.repo_name).count();
            println!("  {} ({} commits)", c.repo_name.clone().bold(), repo_count);
            current_repo = c.repo_name.clone();
        }
        println!("    {}  {}", c.hash.clone().dark_grey(), c.message);
    }
}

pub fn render_markdown(data: &SummaryData) -> String {
    let mut out = format!("# {}\n\n", data.date_label);

    if let Some(ref summary) = data.ai_summary {
        out.push_str(summary);
    } else {
        let mut current_repo = String::new();
        for c in &data.commits {
            if c.repo_name != current_repo {
                out.push_str(&format!("\n## {} ({} commits)\n\n",
                    c.repo_name,
                    data.commits.iter().filter(|x| x.repo_name == c.repo_name).count()
                ));
                current_repo = c.repo_name.clone();
            }
            out.push_str(&format!("- `{}` {}\n", c.hash, c.message));
        }
    }

    out.push_str(&format!(
        "\n---\n{} commits across {} projects | First: {} · Last: {} | Most active: {} ({})\n",
        data.total_commits, data.project_count,
        data.first_commit_time, data.last_commit_time,
        data.most_active_project, data.most_active_count
    ));

    out
}

pub fn render_json(data: &SummaryData) -> String {
    let json = serde_json::json!({
        "date": data.date_label,
        "total_commits": data.total_commits,
        "project_count": data.project_count,
        "first_commit": data.first_commit_time,
        "last_commit": data.last_commit_time,
        "most_active": {
            "project": data.most_active_project,
            "commits": data.most_active_count,
        },
        "commits": data.commits,
    });
    serde_json::to_string_pretty(&json).unwrap_or_default()
}
```

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: Compiles.

**Step 3: Commit**

```bash
git add src/render.rs src/main.rs
git commit -m "feat: add terminal, markdown, and JSON output renderers"
```

---

### Task 10: Wire Everything Together in main.rs

**Files:**
- Modify: `src/main.rs`

**Step 1: Implement the summary command flow**

Update `main.rs` to wire all modules together:

```rust
mod ai;
mod config;
mod db;
mod hook;
mod init;
mod paths;
mod render;

use anyhow::Result;
use chrono::{Local, Datelike, NaiveDate, Utc};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "diddo", about = "Track your git commits, get AI-powered daily summaries")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(long)]
    md: bool,

    #[arg(long)]
    raw: bool,

    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum Commands {
    Yesterday,
    Week,
    Init,
    Uninstall,
    Hook,
    Config,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = paths::AppPaths::new()?;

    match cli.command {
        Some(Commands::Hook) => {
            let db = db::Database::open(&paths.db_path)?;
            hook::run(&db)?;
        }
        Some(Commands::Init) => {
            init::install(&paths)?;
        }
        Some(Commands::Uninstall) => {
            init::uninstall()?;
        }
        Some(Commands::Config) => {
            println!("Config file: {}", paths.config_path.display());
            println!("Database:    {}", paths.db_path.display());
            println!("Hooks dir:   {}", paths.hooks_dir.display());
        }
        ref cmd => {
            let today = Local::now().date_naive();
            let (from, to, label, period) = match cmd {
                Some(Commands::Yesterday) => {
                    let d = today.pred_opt().unwrap();
                    (d, d, format_date(d), "yesterday".to_string())
                }
                Some(Commands::Week) => {
                    let weekday = today.weekday().num_days_from_monday();
                    let monday = today - chrono::Duration::days(weekday as i64);
                    (monday, today, format!("Week of {}", format_date(monday)), "this week".to_string())
                }
                _ => (today, today, format_date(today), "today".to_string()),
            };

            let db = db::Database::open(&paths.db_path)?;
            let commits = if from == to {
                db.query_date(from)?
            } else {
                db.query_date_range(from, to)?
            };

            if commits.is_empty() {
                println!("\n  No commits found for {}.\n", period);
                return Ok(());
            }

            let cfg = config::AppConfig::load(&paths.config_path)?;
            let ai_summary = if cli.raw {
                None
            } else {
                match ai::get_provider(&cfg) {
                    Some(provider) => {
                        match provider.summarize(&commits, &period) {
                            Ok(s) => Some(s),
                            Err(e) => {
                                eprintln!("AI summary failed ({}), falling back to raw", e);
                                None
                            }
                        }
                    }
                    None => {
                        if !cli.json {
                            eprintln!("  Hint: Install claude or set an API key for AI summaries.\n");
                        }
                        None
                    }
                }
            };

            let mut repo_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
            for c in &commits {
                *repo_counts.entry(c.repo_name.clone()).or_default() += 1;
            }
            let (most_active, most_count) = repo_counts.iter()
                .max_by_key(|(_, v)| *v)
                .map(|(k, v)| (k.clone(), *v))
                .unwrap_or_default();

            let data = render::SummaryData {
                date_label: label,
                ai_summary,
                commits: commits.clone(),
                total_commits: commits.len(),
                project_count: repo_counts.len(),
                first_commit_time: commits.first()
                    .map(|c| c.committed_at.with_timezone(&chrono::Local).format("%H:%M").to_string())
                    .unwrap_or_default(),
                last_commit_time: commits.last()
                    .map(|c| c.committed_at.with_timezone(&chrono::Local).format("%H:%M").to_string())
                    .unwrap_or_default(),
                most_active_project: most_active,
                most_active_count: most_count,
            };

            if cli.json {
                println!("{}", render::render_json(&data));
            } else if cli.md {
                println!("{}", render::render_markdown(&data));
            } else {
                render::render_terminal(&data);
            }
        }
    }

    Ok(())
}

fn format_date(date: NaiveDate) -> String {
    date.format("%A, %b %d, %Y").to_string()
}
```

**Step 2: Verify it compiles and all tests pass**

Run: `cargo build`
Expected: Compiles.

Run: `cargo test`
Expected: All tests PASS.

**Step 3: Test the full flow manually**

Run: `cargo run -- init` — should install hook.

Make a test commit, then run: `cargo run` — should show today's summary (raw if no AI available).

Run: `cargo run -- --raw` — should show raw grouped commits.

Run: `cargo run -- uninstall` — should clean up.

**Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire all modules together — complete diddo CLI"
```

---

### Task 11: README & Final Polish

**Files:**
- Create: `README.md`

**Step 1: Write README**

```markdown
# diddo

Track your git commits, get AI-powered daily summaries.

## Install

```bash
cargo install --path .
```

## Setup

```bash
diddo init
```

This installs a global `post-commit` hook that silently records every commit you make.

## Usage

```bash
diddo              # What did I do today?
diddo yesterday    # What about yesterday?
diddo week         # This week's summary

diddo --raw        # Skip AI, show raw commits
diddo --md         # Output as markdown
diddo --json       # Output as JSON
```

## AI Providers

diddo tries these in order:

1. **Claude Code CLI** (`claude`) — zero config if installed
2. **Codex CLI** (`codex`) — zero config if installed
3. **Direct API** — set in `~/.config/diddo/config.toml`
4. **No AI** — falls back to raw grouped commits

### Config file

```toml
[ai]
provider = "openai"       # "openai" | "anthropic"
api_key = "sk-..."
model = "gpt-4o-mini"

[ai.cli]
prefer = "claude"         # force a specific CLI tool
```

Or use environment variables: `DIDDO_OPENAI_KEY`, `DIDDO_ANTHROPIC_KEY`.

## Uninstall

```bash
diddo uninstall
```

## License

MIT
```

**Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add README with install, setup, and usage instructions"
```
