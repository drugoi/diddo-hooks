# Code Review: Table Output PR

**Focus: Tests and Corner Cases**

## Summary

The table output feature is well-integrated with solid test coverage. A few gaps and one test bug were identified.

---

## Test Bug (Should Fix)

### `renders_useful_empty_period_messages_for_all_output_formats` uses wrong invocation

**Location:** `src/main.rs:1307-1312`

The test uses `parse_cli(["diddo", "--table"])`, which returns `ParsedCli { command: None, summary: SummaryArgs::default() }`. With no subcommand, `SummaryArgs::default()` has `table: false`, so the "table" branch is never exercised.

**Fix:** Use `parse_cli(["diddo", "today", "--table"])` so the test actually gets `SummaryArgs { table: true }` and verifies the table format empty message.

```rust
// Before (buggy - tests terminal format, not table)
summary_request_from_cli(parse_cli(["diddo", "--table"]).unwrap()).unwrap().1

// After (correct)
summary_request_from_cli(parse_cli(["diddo", "today", "--table"]).unwrap()).unwrap().1
```

---

## Corner Cases & Gaps

### 1. **Empty commits with `--table`**

**Status:** ✅ Handled correctly

`render_summary_output` returns early when `commits.is_empty()` and uses `render_empty_summary`, so `render_table` is never called with empty commits. The table format empty message is "No commits recorded for {date_label}." (same as terminal).

### 2. **Division by zero in `repo_table_rows`**

**Status:** ✅ Safe

When `commits` is empty, `grouped_commits` returns `[]`, so no rows are produced and the percentage calculation is never run. When there are commits, `total_commits > 0`. No division-by-zero risk.

### 3. **Percentage rounding (e.g. 33.3% + 33.3% + 33.3% = 99.9%)**

**Status:** ⚠️ Minor UX quirk

With 3 repos and 1 commit each, percentages show 33.3%, 33.3%, 33.3% while the Total row shows 100.0%. This is a common pattern and acceptable. No change suggested.

### 4. **Same repo name, different paths**

**Status:** ✅ Handled

`repo_name_counts` counts distinct `(repo_name, repo_path)` pairs that share the same `repo_name`. When there are multiple paths for the same name, the table shows `repo_name (path)` to disambiguate. Logic in `repo_table_rows` is correct.

### 5. **Single-repo table**

**Status:** ⚠️ Not explicitly tested

`table_output_renders_repo_totals_without_ai` uses 2 repos. A single-repo case would show 1 row and 100.0%. Low risk; optional to add.

### 6. **Table with `yesterday`, `week`, `standup`**

**Status:** ⚠️ Not explicitly tested

`table_output_renders_repo_totals_without_ai` only uses `SummaryPeriod::Today`. Table output should behave the same for other periods; adding tests for `yesterday --table`, `week --table`, `standup --table` would improve coverage.

### 7. **`render_table` / `render_terminal_table_body` with empty commits (direct call)**

**Status:** ⚠️ Theoretical edge case

If `render_table` or `render_terminal_table_body` were called directly with `[]`, they would produce a table with header, no data rows, and a Total row of "0 | 100.0%". The main flow never does this. Defensive handling is optional.

### 8. **Mutually exclusive flags**

**Status:** ✅ Well tested

`rejects_table_with_other_summary_output_flags` covers `--table` with `--md`, `--json`, `--raw`. ArgGroup ensures mutual exclusivity.

### 9. **Raw mode footer preservation**

**Status:** ✅ Tested

`raw_output_does_not_append_repo_table` checks that raw output keeps the full footer (commit count, first/last, most active) and does not include the table.

### 10. **Default output includes table**

**Status:** ✅ Tested

`default_terminal_output_appends_repo_table_after_ai_summary` and `markdown_output_appends_repo_table_by_default` verify that terminal and markdown outputs include the table by default.

---

## Recommendations

1. **Fix the test bug** in `renders_useful_empty_period_messages_for_all_output_formats` (use `diddo today --table`).
2. **Optional:** Add a test for table output with a single repo.
3. **Optional:** Add tests for `yesterday --table`, `week --table`, `standup --table` to cover all summary periods.
4. **Optional:** Add a test that `render_empty_summary` with `table: true` returns the expected message (after fixing the existing test).

---

## Overall

The implementation is solid, and the test suite covers the main paths. The only required change is fixing the empty-period test to actually exercise the table format.
