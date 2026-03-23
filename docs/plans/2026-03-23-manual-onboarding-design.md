# diddo manual onboarding — Design Document

Manual onboarding imports existing commit history from a repository into `diddo`'s local database without changing how future hook-based tracking works.

## Scope

- **Command:** `diddo onboard`
- **Primary goal:** backfill commit history for an existing repository from a configurable cutoff date forward
- **Import filter:** only import commits authored by identities that belong to the current user
- **Identity source:** preselect the current repo git identity, optionally include reusable aliases saved in config, and allow the user to confirm additional detected identities
- **Out of scope:**
  - replacing `diddo init`
  - importing all authors by default
  - changing summary query behavior
  - keeping a separate imported-history table

## Recommended approach

Add a dedicated onboarding command instead of extending `diddo init`.

Why this approach:

- `diddo init` already handles hook installation and local/global hook edge cases; keeping onboarding separate avoids mixing setup with historical import
- backfilling old history is a user-driven, higher-risk action that benefits from an explicit command and confirmation flow
- a standalone command can later support both interactive and scripted usage without complicating the main setup path

## User flow

1. User runs `diddo onboard` inside an existing git repository.
2. `diddo` asks for a cutoff date, meaning "import commits on or after this date".
3. `diddo` scans git history in that range and detects author names and emails.
4. `diddo` preselects the current repo identity from git config and any saved onboarding aliases from config.
5. `diddo` shows detected identities and asks the user which ones should count as their own history.
6. `diddo` imports only commits whose author matches one of the selected identities.
7. `diddo` reports scan and import totals, including duplicates skipped because they already exist in the database.
8. If the user selected new identities, `diddo` offers to save them as reusable aliases for future onboarding.

## Command shape

Initial command:

- `diddo onboard`

Possible follow-up flags for later iterations:

- `--from <date>` to bypass the cutoff prompt
- `--yes` for non-interactive confirmation
- `--author-email <email>` for scripted imports

The default path should remain interactive because identity confirmation is the key safety step.

## Architecture

### New module

Add a new module such as `src/onboarding.rs` responsible for:

- validating repository context
- collecting onboarding inputs
- scanning git history from the cutoff date forward
- resolving author identity matches
- converting git history into `db::Commit` records
- importing records through the existing database insertion path

### Existing database reuse

Reuse the current `commits` table and `db::Commit` model.

Why:

- the existing unique index on `(repo_path, hash)` already gives safe re-import behavior
- imported commits should behave exactly like hook-recorded commits in summaries and metadata
- avoiding a second history table keeps queries, rendering, and AI summaries unchanged

### Separation of concerns

Keep these layers separate:

- **Prompt/UI flow:** collect cutoff date, show identities, confirm save behavior
- **Git scan logic:** enumerate commit metadata and detected author identities
- **Import logic:** filter matching commits and write them to the database
- **Config persistence:** load and optionally save onboarding aliases

This separation makes the feature easier to test and allows non-interactive onboarding later without rewriting the core logic.

## Data model and config

Do not store identity aliases in the database. Store them in config instead because they represent user preference rather than commit facts.

Proposed config shape:

```toml
[onboarding]
save_selected_identities = true

[[onboarding.identity_aliases]]
name = "Nikita Bayev"
email = "nikita@old-company.com"

[[onboarding.identity_aliases]]
email = "drugoi@example.com"
```

Proposed config behavior:

- load aliases during onboarding and include them in the candidate identity set
- when the user selects newly detected identities, offer to persist them
- treat config-save failures as warnings if the import itself already succeeded

## Identity matching

### Matching rules

- Prefer email-based matching whenever an email is present.
- Allow name-based selection only as a fallback for commits missing author email.
- Preselect the current repo git identity from `git config user.email` and, if useful, `git config user.name`.
- Merge saved aliases from config into the selected set before prompting.
- Show detected identities from the scanned date range so the user can explicitly confirm extra matches.

### Rationale

- emails are more precise and stable than names
- users may have older company or personal emails that should still map to the same person
- requiring confirmation avoids accidentally importing another contributor's work in shared repositories

## Data flow

1. Verify the current directory is a git repository.
2. Ask for or read the cutoff date.
3. Scan git history on or after that date and collect:
   - commit hash
   - commit message/subject
   - author name
   - author email
   - commit timestamp
4. Build the candidate identity set from:
   - current repo git identity
   - saved onboarding aliases
   - identities detected in the scanned history
5. Ask the user to confirm which identities belong to them.
6. Filter scanned commits to only matching identities.
7. Convert filtered commits into existing `db::Commit` rows.
8. Insert them using `insert_commit()`.
9. Report:
   - total commits scanned
   - matching commits found
   - newly inserted commits
   - duplicates skipped
10. Offer to save newly selected identities into config.

## Error handling

| Situation | Behavior |
|-----------|----------|
| Not in a git repo | Fail fast with a clear onboarding-specific error message. |
| Invalid cutoff date | Re-prompt in interactive mode or return a CLI parse error in flag-driven mode. |
| No commits on or after cutoff | Print a friendly "nothing to import" message and exit successfully. |
| No matching identities detected | Show detected identities and require explicit user selection instead of guessing. |
| Some commits have no author email | Allow manual name-based selection, but message that it is less precise. |
| Config save fails after successful import | Print a warning and keep the imported commits. |
| Import interrupted midway | Safe to rerun because duplicate commits are deduped by `(repo_path, hash)`. |

## Testing

### CLI tests

- parse `diddo onboard`
- parse future optional flags such as `--from`
- reject invalid dates when flags are used

### Identity resolution tests

- current git identity only
- current identity plus saved aliases
- overlapping or duplicate aliases
- email-first matching with name-based fallback
- explicit manual selection when no auto-match exists

### Import logic tests

Use `db::Database::open_in_memory()`:

- imports only commits on or after the cutoff date
- imports only commits whose identity matches the selected set
- skips duplicates on rerun
- handles mixed-author history correctly

### Integration-style onboarding tests

- repo validation
- empty history after cutoff
- prompt and selection behavior
- alias persistence into config

## Success criteria

- Users can backfill history for an existing repo without reinstalling hooks.
- The cutoff date clearly means "import commits on or after this date".
- Only commits belonging to the current user are imported.
- Users can review and expand identity matching when they have used multiple names or emails.
- Re-running onboarding is safe and does not create duplicate commit rows.
- Imported commits appear in existing summaries and metadata without any special-case query logic.
