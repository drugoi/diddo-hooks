# Manual Onboarding Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a `diddo onboard` command that imports commits from the current repository on or after a cutoff date, limited to user-selected author identities that belong to the current user.

**Architecture:** Keep onboarding separate from hook setup and summary rendering. Add a dedicated `src/onboarding.rs` module for git scanning, identity resolution, and import orchestration; keep prompt flow, import logic, and config persistence separated so the core import logic remains easy to test.

**Tech Stack:** Rust, clap, rusqlite, chrono, existing `diddo` config and path helpers, git CLI subprocesses, in-memory database tests.

---

### Task 1: Add CLI surface for onboarding

**Files:**
- Modify: `src/main.rs`
- Test: `src/main.rs`

**Step 1: Write the failing CLI parsing tests**

Add tests near the existing CLI parsing tests in `src/main.rs` for:

```rust
#[test]
fn parses_onboard_subcommand() {
    let cli = parse_cli(["diddo", "onboard"]).unwrap();

    assert_eq!(cli.command, Some(Commands::Onboard));
}

#[test]
fn rejects_summary_flags_on_onboard_command() {
    let error = parse_cli(["diddo", "onboard", "--md"]).unwrap_err();

    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test parses_onboard_subcommand rejects_summary_flags_on_onboard_command`

Expected: FAIL because `Commands::Onboard` does not exist yet.

**Step 3: Write minimal implementation**

In `src/main.rs`:

- add `mod onboarding;`
- add `Onboard` to `Commands`
- update main command dispatch so `Some(Commands::Onboard)` calls a new `run_onboard_command()` helper
- keep onboarding outside summary argument parsing

Minimal shape:

```rust
#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
enum Commands {
    // ...
    /// Import existing history for the current repository.
    Onboard,
}

fn run_onboard_command() -> Result<(), Box<dyn Error>> {
    let paths = paths::AppPaths::new()?;
    let database = db::Database::open(&paths.db_path)?;
    let config = config::AppConfig::load(&paths.config_path)?;

    onboarding::run(&database, &paths.config_path, config)
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test parses_onboard_subcommand rejects_summary_flags_on_onboard_command`

Expected: PASS

**Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: add onboarding command entrypoint"
```

### Task 2: Add onboarding config model and persistence helpers

**Files:**
- Modify: `src/config.rs`
- Test: `src/config.rs`

**Step 1: Write the failing config tests**

Add tests in `src/config.rs` for:

```rust
#[test]
fn parses_onboarding_identity_aliases_from_toml() {
    let config = AppConfig::load(&config_path).unwrap();

    assert_eq!(config.onboarding.identity_aliases.len(), 2);
    assert_eq!(
        config.onboarding.identity_aliases[0].email.as_deref(),
        Some("nikita@old-company.com")
    );
}

#[test]
fn save_onboarding_aliases_writes_expected_toml() {
    save_onboarding_aliases(&config_path, &aliases).unwrap();

    let written = fs::read_to_string(&config_path).unwrap();
    assert!(written.contains("[onboarding]"));
    assert!(written.contains("[[onboarding.identity_aliases]]"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test parses_onboarding_identity_aliases_from_toml save_onboarding_aliases_writes_expected_toml`

Expected: FAIL because onboarding config structs and save helpers do not exist.

**Step 3: Write minimal implementation**

In `src/config.rs`:

- extend `AppConfig` with `pub onboarding: OnboardingConfig`
- add:

```rust
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct OnboardingConfig {
    pub save_selected_identities: bool,
    pub identity_aliases: Vec<IdentityAlias>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct IdentityAlias {
    pub name: Option<String>,
    pub email: Option<String>,
}
```

- implement `Default` for `OnboardingConfig` with `save_selected_identities: true`
- add a small write helper that persists onboarding aliases without disturbing unrelated config sections more than necessary
- keep normalization simple: trim empty names/emails to `None`

**Step 4: Run test to verify it passes**

Run: `cargo test parses_onboarding_identity_aliases_from_toml save_onboarding_aliases_writes_expected_toml`

Expected: PASS

**Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: add onboarding identity alias config"
```

### Task 3: Build onboarding core types and pure import logic

**Files:**
- Create: `src/onboarding.rs`
- Modify: `src/main.rs`
- Test: `src/onboarding.rs`

**Step 1: Write the failing pure-logic tests**

Add tests in `src/onboarding.rs` for:

```rust
#[test]
fn filters_commits_on_or_after_cutoff_date() {
    let imported = filter_importable_commits(&commits, cutoff, &selected_identities);

    assert_eq!(imported.iter().map(|c| c.hash.as_str()).collect::<Vec<_>>(), vec!["bbb2222"]);
}

#[test]
fn imports_only_selected_identities() {
    let imported = filter_importable_commits(&commits, cutoff, &selected_identities);

    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0].author_email.as_deref(), Some("me@example.com"));
}

#[test]
fn import_is_idempotent_when_re_run() {
    import_commits(&database, &repo_path, &repo_name, &filtered).unwrap();
    import_commits(&database, &repo_path, &repo_name, &filtered).unwrap();

    assert_eq!(database.commit_count().unwrap(), 1);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test filters_commits_on_or_after_cutoff_date imports_only_selected_identities import_is_idempotent_when_re_run`

Expected: FAIL because onboarding module and helpers do not exist.

**Step 3: Write minimal implementation**

Create `src/onboarding.rs` with:

- `ScannedCommit` struct containing hash, message, author name, author email, branch placeholder if needed, and committed timestamp
- `IdentityCandidate` struct for name/email combinations
- pure helpers:
  - `filter_importable_commits(...)`
  - `identity_matches(...)`
  - `to_db_commit(...)`
  - `import_commits(...)`
- summary result struct:

```rust
pub struct ImportOutcome {
    pub scanned: usize,
    pub matched: usize,
    pub inserted: usize,
    pub skipped_duplicates: usize,
}
```

Implementation rules:

- cutoff is inclusive: import commits where `committed_at.date_naive() >= cutoff`
- prefer email matches; allow name fallback only when email is missing
- write through existing `db::Database::insert_commit()`
- compute skipped duplicates by comparing count before and after import, or by tracking attempted rows versus resulting unique rows

**Step 4: Run test to verify it passes**

Run: `cargo test filters_commits_on_or_after_cutoff_date imports_only_selected_identities import_is_idempotent_when_re_run`

Expected: PASS

**Step 5: Commit**

```bash
git add src/onboarding.rs src/main.rs
git commit -m "feat: add onboarding import core"
```

### Task 4: Add git scan and identity detection adapters

**Files:**
- Modify: `src/onboarding.rs`
- Test: `src/onboarding.rs`

**Step 1: Write the failing adapter tests**

Add tests that cover:

```rust
#[test]
fn parse_git_log_output_builds_scanned_commits() {
    let commits = parse_git_log_output(sample_log()).unwrap();

    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].hash, "abc1234");
}

#[test]
fn detected_identities_include_unique_name_email_pairs() {
    let identities = detect_identities(&commits);

    assert_eq!(identities.len(), 2);
    assert!(identities.iter().any(|i| i.email.as_deref() == Some("me@example.com")));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test parse_git_log_output_builds_scanned_commits detected_identities_include_unique_name_email_pairs`

Expected: FAIL because git log parsing and identity detection helpers are not implemented.

**Step 3: Write minimal implementation**

In `src/onboarding.rs`:

- add a git adapter function that shells out to `git log`
- use a delimiter-safe format such as `%H%x1f%s%x1f%an%x1f%ae%x1f%cI`
- parse each record into `ScannedCommit`
- add `detect_identities(&[ScannedCommit]) -> Vec<IdentityCandidate>`
- sort identities predictably, preferring email-backed identities first, then by email/name text
- inject the git command runner in tests rather than invoking the real repo

**Step 4: Run test to verify it passes**

Run: `cargo test parse_git_log_output_builds_scanned_commits detected_identities_include_unique_name_email_pairs`

Expected: PASS

**Step 5: Commit**

```bash
git add src/onboarding.rs
git commit -m "feat: add onboarding git history scan"
```

### Task 5: Wire current identity resolution and alias preselection

**Files:**
- Modify: `src/onboarding.rs`
- Modify: `src/config.rs`
- Test: `src/onboarding.rs`

**Step 1: Write the failing identity resolution tests**

Add tests for:

```rust
#[test]
fn preselects_current_git_email_and_saved_aliases() {
    let selected = build_preselected_identities(current_identity, &saved_aliases, &detected);

    assert!(selected.iter().any(|i| i.email.as_deref() == Some("me@example.com")));
    assert!(selected.iter().any(|i| i.email.as_deref() == Some("me@old-company.com")));
}

#[test]
fn name_only_matches_are_not_auto_selected_when_email_candidates_exist() {
    let selected = build_preselected_identities(current_identity, &saved_aliases, &detected);

    assert!(!selected.iter().any(|i| i.name.as_deref() == Some("Nikita")));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test preselects_current_git_email_and_saved_aliases name_only_matches_are_not_auto_selected_when_email_candidates_exist`

Expected: FAIL because preselection logic does not exist yet.

**Step 3: Write minimal implementation**

In `src/onboarding.rs`:

- add a helper to read current repo identity from `git config user.email` and `git config user.name`
- add `build_preselected_identities(...)`
- combine:
  - current repo identity
  - saved aliases from config
  - detected identities from scanned commits
- auto-select exact email matches first
- keep name-only candidates visible but unselected unless the user confirms them

**Step 4: Run test to verify it passes**

Run: `cargo test preselects_current_git_email_and_saved_aliases name_only_matches_are_not_auto_selected_when_email_candidates_exist`

Expected: PASS

**Step 5: Commit**

```bash
git add src/onboarding.rs src/config.rs
git commit -m "feat: preselect onboarding identities"
```

### Task 6: Add interactive onboarding orchestration

**Files:**
- Modify: `src/onboarding.rs`
- Modify: `src/main.rs`
- Test: `src/onboarding.rs`

**Step 1: Write the failing orchestration tests**

Add tests for:

```rust
#[test]
fn returns_nothing_to_import_when_cutoff_has_no_commits() {
    let outcome = run_with(deps).unwrap();

    assert_eq!(outcome.inserted, 0);
    assert_eq!(outcome.scanned, 0);
}

#[test]
fn onboarding_run_imports_only_user_confirmed_identities() {
    let outcome = run_with(deps).unwrap();

    assert_eq!(outcome.scanned, 3);
    assert_eq!(outcome.matched, 2);
    assert_eq!(outcome.inserted, 2);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test returns_nothing_to_import_when_cutoff_has_no_commits onboarding_run_imports_only_user_confirmed_identities`

Expected: FAIL because the end-to-end onboarding runner is not wired.

**Step 3: Write minimal implementation**

In `src/onboarding.rs`:

- expose `pub fn run(...) -> Result<(), Box<dyn Error>>`
- split orchestration into injected helpers for:
  - reading cutoff date
  - reading git identity
  - scanning history
  - selecting identities
  - saving aliases
- print friendly terminal messages for:
  - repo validation failure
  - empty history after cutoff
  - import summary counts
- keep UI simple in v1:
  - prompt for cutoff date in `YYYY-MM-DD` or `DD.MM.YYYY`
  - print numbered identity candidates
  - accept comma-separated selections
  - ask whether to save newly selected aliases

**Step 4: Run test to verify it passes**

Run: `cargo test returns_nothing_to_import_when_cutoff_has_no_commits onboarding_run_imports_only_user_confirmed_identities`

Expected: PASS

**Step 5: Commit**

```bash
git add src/onboarding.rs src/main.rs
git commit -m "feat: add interactive onboarding flow"
```

### Task 7: Document the feature and verify the full suite

**Files:**
- Modify: `README.md`
- Modify: `docs/plans/2026-03-23-manual-onboarding-design.md`
- Test: `src/main.rs`, `src/config.rs`, `src/onboarding.rs`

**Step 1: Write the failing doc-adjacent tests or assertions if needed**

If README examples require behavior validation, add or update a lightweight CLI test in `src/main.rs` that asserts the command help includes onboarding.

Example:

```rust
#[test]
fn help_output_mentions_onboard_command() {
    let help = HelpCli::command().render_long_help().to_string();

    assert!(help.contains("onboard"));
}
```

**Step 2: Run targeted verification**

Run: `cargo test help_output_mentions_onboard_command`

Expected: PASS after command help is updated.

**Step 3: Update docs**

- add `diddo onboard` to `README.md`
- explain that `diddo` normally records only future commits, while onboarding imports older history for the current repo
- document cutoff semantics clearly: "on or after this date"
- document identity confirmation and optional alias reuse

**Step 4: Run full verification**

Run: `cargo fmt -- --check`
Expected: PASS

Run: `cargo test`
Expected: PASS with all tests green

Run: `cargo run -- --help`
Expected: help output includes `onboard`

**Step 5: Commit**

```bash
git add README.md src/main.rs src/config.rs src/onboarding.rs docs/plans/2026-03-23-manual-onboarding-design.md
git commit -m "feat: add manual repository onboarding"
```
