# Agent memory

## Project overview

**diddo** is a Rust CLI that tracks git commits and produces AI-powered daily summaries. The repo name is `diddo-hooks`; the binary and crate name is `diddo`.

- **Behavior:** Installs a global `post-commit` hook, stores commit metadata in a local SQLite database, and can summarize that history via AI CLI tools (claude, codex, opencode, cursor) or direct API (OpenAI, Anthropic).
- **Layout:** Entrypoint and summary flow in `src/main.rs`; hook logic in `src/hook.rs`; AI provider selection and prompt building in `src/ai/` (`mod.rs`, `cli_provider.rs`, `api_provider.rs`); persistence in `src/db.rs`; config in `src/config.rs`; paths in `src/paths.rs`; rendering in `src/render.rs`; grouping in `src/summary_group.rs`; init/uninstall in `src/init.rs`. Design and implementation notes live in `docs/plans/`.
- **Verification:** Run `cargo test` for the full suite. Use `cargo run -- --help` to confirm CLI behavior without installing. Releases: use `cargo release patch` (or `minor`/exact version); then push the tag to trigger the GitHub Actions workflow (`v*`).

## Build & Test Commands

```bash
cargo build --release          # Build optimized binary
cargo test                     # Run all tests
cargo test <test_name>         # Run a single test by name
cargo test --lib               # Unit tests only
cargo run -- --help            # Run from source
cargo install --path .         # Install locally
cargo clippy                   # Lint
cargo fmt -- --check           # Check formatting
```

## Learned User Preferences

- Prefers US English for standard repo files (e.g. LICENSE, not LICENCE).
- Prefers `.md` extension for repo-root documentation files (e.g. `LICENSE.md`).

## Learned Workspace Facts

- When a CLI subcommand is missing or behavior looks outdated after `cargo install --path .`, check which binary runs (e.g. `which diddo`); if `~/.local/bin` precedes `~/.cargo/bin` in PATH, the shell may be running an older binary—reinstall from the desired path or put `~/.cargo/bin` first in PATH.
- Tests avoid real I/O via dependency injection: `hook::run_with` injects git and diff-stat functions; summary tests inject config loader and `create_provider`. Use `db::Database::open_in_memory()` in tests instead of a file path.
- Linux aarch64 cross-compile uses a custom linker; see `.cargo/config.toml` (`linker = "aarch64-linux-gnu-gcc"`). The release workflow installs `gcc-aarch64-linux-gnu` on the runner for that target.
- Homebrew distribution uses a separate `drugoi/homebrew-diddo` tap repo. The formula is at `Formula/diddo.rb` and references pre-built tarballs from `diddo-hooks` GitHub Releases.
- The project copyright holder is Nikita Bayev (MIT license, 2026).
