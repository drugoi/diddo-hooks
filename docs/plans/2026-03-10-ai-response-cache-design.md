# AI response cache design

**Date:** 2026-03-10

## Summary

Cache AI summaries when commit data and period are unchanged. Cache key includes provider and model so we never reuse a summary from a different provider/model. Storage is a new table in the existing SQLite DB; no TTL.

## Decisions

| Topic | Choice |
|-------|--------|
| Cache key scope | Include provider and model (no cross-provider/model reuse) |
| Storage | New table in existing `commits.db` |
| TTL | None — entries stay until input (period + commits + provider/model) changes |
| Input identity | Hash of the actual prompt text (so "same commits" ⇒ same prompt ⇒ cache hit) |
| Force refresh | Optional `--no-cache` flag to skip cache and overwrite |

## Architecture

- **Lookup:** Before calling any AI provider, build the prompt with `build_prompt(commits, period)`, then compute cache key from `(provider_id, model_id, period, prompt_text)` (e.g. SHA-256 hex). Look up in cache; on hit return stored summary and skip the AI call.
- **Storage:** New table in `commits.db`, e.g. `ai_summary_cache (cache_key TEXT PRIMARY KEY, summary TEXT)`. Optional `created_at` for debugging. No TTL.
- **Flow:** In the path that currently calls `try_ai_summary` → provider `summarize()`: resolve which provider (and model) will be used, build prompt, compute key, SELECT summary; on miss call provider then INSERT/REPLACE.
- **Provider/model in key:** Stable provider id (e.g. `claude`, `codex`, `openai`, `anthropic`) and for API providers the configured model name. CLI providers: tool name as provider id, model omitted or fixed (e.g. `default`).

## Schema

- Table: `ai_summary_cache (cache_key TEXT PRIMARY KEY, summary TEXT)` with optional `created_at TEXT`.
- Key: `sha256(provider_id + "\0" + model_id + "\0" + period + "\0" + prompt_text)` encoded as hex (or another stable encoding).

## Edge cases

- **Empty period:** Cache the "no commits" case with the same key logic so repeated `diddo today` with no commits doesn’t call the AI.
- **Fallback provider:** When the first provider fails and fallback succeeds, cache only the successful result under the provider/model that produced it.
- **Schema:** Create the new table in the same place as the existing schema (e.g. `Database::initialize`) so new and existing installs get it without a separate migration.

## Optional: force refresh

- Flag `--no-cache` (or `--refresh`): skip cache lookup, always call AI, then overwrite the cache entry for the resulting key. Recommended for first version.

## Testing

- Unit: same (period, commits, provider, model) ⇒ cache hit, no provider call.
- Unit: same commits, different provider or model ⇒ cache miss.
- Unit: different commits or period ⇒ cache miss.
- Integration: run summary twice with same data ⇒ second run returns cached summary; with `--no-cache` bypasses cache.
