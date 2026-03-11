## Cursor Cloud specific instructions

This is a pure Rust CLI project (`diddo`) — no web services, Docker, or databases to run externally. SQLite is compiled in via `rusqlite` with the `bundled` feature.

### Key commands

See `README.md` for full usage. Quick reference:

- **Build:** `cargo build`
- **Test:** `cargo test` (93 unit tests, all in-process, no external deps)
- **Lint:** `cargo clippy` (note: the codebase has ~6 pre-existing clippy warnings)
- **Run:** `cargo run -- <subcommand>` (e.g. `cargo run -- today --raw`)

### Rust version requirement

The project uses `edition = "2024"` which requires Rust >= 1.85. The VM ships with Rust 1.83 pinned as default. The update script runs `rustup default stable` to ensure the latest stable is active. If you see `feature edition2024 is required`, run `rustup default stable`.

### Testing the hook flow end-to-end

1. `cargo run -- init` — installs a global `post-commit` git hook
2. Make a commit in any repo (e.g. a temp repo in `/tmp`)
3. `cargo run -- today --raw` — shows today's recorded commits

AI summary features require either an AI CLI tool (`claude`, `codex`, etc.) or API keys (`DIDDO_OPENAI_KEY` / `DIDDO_ANTHROPIC_KEY`). Without these, `diddo` falls back to raw commit output, which is fine for testing.
