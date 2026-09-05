use std::{fs, path::Path};

use chrono::{DateTime, Local, LocalResult, NaiveDate, TimeZone, Utc};
use rusqlite::{Connection, Result, Row, params, types::Type};

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub author_email: Option<String>,
}

#[allow(dead_code)]
const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS commits (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        hash TEXT NOT NULL,
        message TEXT NOT NULL,
        repo_path TEXT NOT NULL,
        repo_name TEXT NOT NULL,
        branch TEXT NOT NULL,
        files_changed INTEGER NOT NULL DEFAULT 0,
        insertions INTEGER NOT NULL DEFAULT 0,
        deletions INTEGER NOT NULL DEFAULT 0,
        committed_at TEXT NOT NULL,
        author_email TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_commits_date_repo
        ON commits (committed_at, repo_name);
    CREATE UNIQUE INDEX IF NOT EXISTS idx_commits_repo_path_hash
        ON commits (repo_path, hash);
    CREATE TABLE IF NOT EXISTS ai_summary_cache (
        cache_key TEXT PRIMARY KEY,
        summary TEXT NOT NULL
    );
";

#[allow(dead_code)]
pub struct Database {
    connection: Connection,
}

#[allow(dead_code)]
impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|error| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                    Some(format!(
                        "could not create data directory {}: {error}",
                        parent.display()
                    )),
                )
            })?;
        }

        let connection = Connection::open(path)?;
        Self::initialize(connection)
    }

    pub fn open_in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        Self::initialize(connection)
    }

    fn initialize(connection: Connection) -> Result<Self> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;

        // WAL lets a reader coexist with the hook's writer and removes the
        // rollback-journal create/delete churn on every commit. It can legitimately
        // fail on filesystems without shared memory (NFS/SMB home directories), and
        // on in-memory databases it simply reports "memory" — diddo must keep working
        // in both cases, so a non-WAL outcome is tolerated rather than fatal.
        let journal_mode = connection
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))
            .unwrap_or_default();

        // synchronous=NORMAL is corruption-safe ONLY in WAL mode. SQLite documents a
        // small but non-zero chance of corruption on power loss when NORMAL is combined
        // with a rollback journal, so if WAL did not take effect leave synchronous at
        // its default (FULL) and accept the slower writes.
        if journal_mode.eq_ignore_ascii_case("wal") {
            connection.pragma_update(None, "synchronous", "NORMAL")?;
        }

        const SCHEMA_VERSION: i64 = 1;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version < SCHEMA_VERSION {
            connection.execute_batch(SCHEMA)?;
            run_author_email_migration(&connection)?;
            connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if version > SCHEMA_VERSION {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISMATCH),
                Some(format!(
                    "database schema version {version} is newer than this diddo build supports ({SCHEMA_VERSION}); upgrade diddo (run `diddo update`)"
                )),
            ));
        }

        Ok(Self { connection })
    }

    pub fn insert_commit(&self, commit: &Commit) -> Result<()> {
        self.connection.execute(
            "INSERT INTO commits (
                hash,
                message,
                repo_path,
                repo_name,
                branch,
                files_changed,
                insertions,
                deletions,
                committed_at,
                author_email
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(repo_path, hash) DO UPDATE SET
                message = excluded.message,
                repo_name = excluded.repo_name,
                branch = excluded.branch,
                files_changed = excluded.files_changed,
                insertions = excluded.insertions,
                deletions = excluded.deletions,
                committed_at = excluded.committed_at,
                author_email = excluded.author_email",
            params![
                &commit.hash,
                &commit.message,
                &commit.repo_path,
                &commit.repo_name,
                &commit.branch,
                commit.files_changed,
                commit.insertions,
                commit.deletions,
                commit.committed_at.to_rfc3339(),
                &commit.author_email,
            ],
        )?;

        Ok(())
    }

    pub fn query_date(&self, date: NaiveDate) -> Result<Vec<Commit>> {
        let (start, end) = date_range_bounds_local(date, date)?;
        self.query_date_range_raw(&start, &end)
    }

    pub fn query_date_range(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<Commit>> {
        let (start, end) = date_range_bounds_local(from, to)?;
        self.query_date_range_raw(&start, &end)
    }

    pub fn query_datetime_range(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<Commit>> {
        self.query_date_range_raw(&from.to_rfc3339(), &to.to_rfc3339())
    }

    fn query_date_range_raw(&self, start: &str, end: &str) -> Result<Vec<Commit>> {
        let mut statement = self.connection.prepare(
            "SELECT id, hash, message, repo_path, repo_name, branch, files_changed, insertions, deletions, committed_at, author_email
             FROM commits
             WHERE committed_at >= ?1 AND committed_at < ?2
             ORDER BY repo_name, committed_at",
        )?;
        let rows = statement.query_map(params![start, end], commit_from_row)?;

        rows.collect()
    }

    pub fn get_cached_summary(&self, cache_key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .connection
            .prepare("SELECT summary FROM ai_summary_cache WHERE cache_key = ?1")?;
        let mut rows = stmt.query(params![cache_key])?;
        if let Some(row) = rows.next()? {
            let summary: String = row.get(0)?;
            return Ok(Some(summary));
        }
        Ok(None)
    }

    pub fn set_cached_summary(&self, cache_key: &str, summary: &str) -> Result<()> {
        self.connection.execute(
            "INSERT OR REPLACE INTO ai_summary_cache (cache_key, summary) VALUES (?1, ?2)",
            params![cache_key, summary],
        )?;
        Ok(())
    }

    pub fn commit_count(&self) -> Result<i64> {
        self.connection
            .query_row("SELECT COUNT(*) FROM commits", [], |row| row.get(0))
    }

    pub fn oldest_commit_date(&self) -> Result<Option<String>> {
        self.connection
            .query_row("SELECT MIN(committed_at) FROM commits", [], |row| {
                row.get(0)
            })
    }

    /// Column names of the commits table.
    pub fn commit_table_column_names(&self) -> Result<Vec<String>> {
        let mut stmt = self.connection.prepare("PRAGMA table_info(commits)")?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>>>()?;
        Ok(names)
    }
}

fn date_range_bounds_local(from: NaiveDate, to: NaiveDate) -> Result<(String, String)> {
    date_range_bounds_in_timezone(from, to, &Local)
}

fn author_email_column_exists(conn: &Connection) -> Result<bool> {
    let mut stmt = conn.prepare("PRAGMA table_info(commits)")?;
    let names: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(names.iter().any(|s| s == "author_email"))
}

fn run_author_email_migration(conn: &Connection) -> Result<()> {
    if !author_email_column_exists(conn)? {
        conn.execute("ALTER TABLE commits ADD COLUMN author_email TEXT", [])?;
    }
    Ok(())
}

fn date_range_bounds_in_timezone<Tz: TimeZone>(
    from: NaiveDate,
    to: NaiveDate,
    timezone: &Tz,
) -> Result<(String, String)> {
    if from > to {
        return Err(rusqlite::Error::InvalidQuery);
    }

    let start = local_day_start_in_utc(from, timezone)?.to_rfc3339();
    let end = local_day_start_in_utc(
        to.succ_opt().ok_or(rusqlite::Error::InvalidQuery)?,
        timezone,
    )?
    .to_rfc3339();

    Ok((start, end))
}

fn local_day_start_in_utc<Tz: TimeZone>(date: NaiveDate, timezone: &Tz) -> Result<DateTime<Utc>> {
    let local_midnight = date
        .and_hms_opt(0, 0, 0)
        .ok_or(rusqlite::Error::InvalidQuery)?;

    match timezone.from_local_datetime(&local_midnight) {
        LocalResult::Single(value) => Ok(value.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, _) => Ok(first.with_timezone(&Utc)),
        // A nonexistent local midnight (DST spring-forward at 00:00, real in e.g.
        // America/Santiago, America/Havana, Asia/Beirut) is walked forward one hour
        // at a time until a wall-clock instant actually exists. A spring-forward gap
        // is at most two hours anywhere on Earth, so trying up to 03:00 is sufficient.
        LocalResult::None => first_resolvable_local_hour(1..=3, |hour| {
            date.and_hms_opt(hour, 0, 0)
                .map(|naive| {
                    timezone
                        .from_local_datetime(&naive)
                        .map(|v| v.with_timezone(&Utc))
                })
                .unwrap_or(LocalResult::None)
        })
        .ok_or(rusqlite::Error::InvalidQuery),
    }
}

/// Tries each hour in `hours` via `resolve`, returning the first wall-clock instant
/// that actually exists (preferring the earlier instant when a resolved hour is
/// itself ambiguous, e.g. a fall-back DST transition). Extracted from
/// `local_day_start_in_utc` so the "walk forward, take the first that resolves"
/// behavior can be unit-tested with constructed `LocalResult` values instead of a
/// hand-rolled `chrono::TimeZone` implementation.
fn first_resolvable_local_hour<F>(
    hours: std::ops::RangeInclusive<u32>,
    mut resolve: F,
) -> Option<DateTime<Utc>>
where
    F: FnMut(u32) -> LocalResult<DateTime<Utc>>,
{
    hours.into_iter().find_map(|hour| match resolve(hour) {
        LocalResult::Single(value) => Some(value),
        LocalResult::Ambiguous(value, _) => Some(value),
        LocalResult::None => None,
    })
}

fn commit_from_row(row: &Row<'_>) -> Result<Commit> {
    let committed_at = row.get::<_, String>(9).and_then(|value| {
        DateTime::parse_from_rfc3339(&value)
            .map(|datetime| datetime.with_timezone(&Utc))
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(9, Type::Text, Box::new(error))
            })
    })?;

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
        author_email: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    use chrono::{Duration, FixedOffset, Local, NaiveDate, TimeZone, Utc};
    use rusqlite::Row;

    use super::{Commit, Database, date_range_bounds_in_timezone};

    #[test]
    fn creates_commits_table_with_expected_columns_and_date_repo_index() {
        let database = Database::open_in_memory().unwrap();

        let table_name = schema_object_name(&database, "table", "commits");
        let table_sql = schema_object_sql(&database, "table", "commits");
        let index_name = schema_object_name(&database, "index", "idx_commits_date_repo");
        let unique_index_name =
            schema_object_name(&database, "index", "idx_commits_repo_path_hash");
        let columns = commit_columns(&database);

        assert_eq!(table_name, "commits");
        assert_eq!(
            normalize_sql(&table_sql),
            normalize_sql(
                "CREATE TABLE commits (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    hash TEXT NOT NULL,
                    message TEXT NOT NULL,
                    repo_path TEXT NOT NULL,
                    repo_name TEXT NOT NULL,
                    branch TEXT NOT NULL,
                    files_changed INTEGER NOT NULL DEFAULT 0,
                    insertions INTEGER NOT NULL DEFAULT 0,
                    deletions INTEGER NOT NULL DEFAULT 0,
                    committed_at TEXT NOT NULL,
                    author_email TEXT
                )"
            )
        );
        assert_eq!(index_name, "idx_commits_date_repo");
        assert_eq!(unique_index_name, "idx_commits_repo_path_hash");
        assert_eq!(
            columns,
            vec![
                ("id".to_string(), "INTEGER".to_string(), false, None, true),
                ("hash".to_string(), "TEXT".to_string(), true, None, false),
                ("message".to_string(), "TEXT".to_string(), true, None, false),
                (
                    "repo_path".to_string(),
                    "TEXT".to_string(),
                    true,
                    None,
                    false
                ),
                (
                    "repo_name".to_string(),
                    "TEXT".to_string(),
                    true,
                    None,
                    false
                ),
                ("branch".to_string(), "TEXT".to_string(), true, None, false),
                (
                    "files_changed".to_string(),
                    "INTEGER".to_string(),
                    true,
                    Some("0".to_string()),
                    false,
                ),
                (
                    "insertions".to_string(),
                    "INTEGER".to_string(),
                    true,
                    Some("0".to_string()),
                    false,
                ),
                (
                    "deletions".to_string(),
                    "INTEGER".to_string(),
                    true,
                    Some("0".to_string()),
                    false,
                ),
                (
                    "committed_at".to_string(),
                    "TEXT".to_string(),
                    true,
                    None,
                    false,
                ),
                (
                    "author_email".to_string(),
                    "TEXT".to_string(),
                    false,
                    None,
                    false
                ),
            ]
        );
    }

    #[test]
    fn open_creates_nested_parent_directories_and_applies_schema() {
        let root = unique_temp_path("diddo-db-open");
        let path = root.join("nested").join("state").join("diddo.sqlite3");

        let database = Database::open(&path).unwrap();
        let index_name = schema_object_name(&database, "index", "idx_commits_date_repo");
        let unique_index_name =
            schema_object_name(&database, "index", "idx_commits_repo_path_hash");

        assert!(path.parent().is_some_and(Path::exists));
        assert!(path.exists());
        assert_eq!(index_name, "idx_commits_date_repo");
        assert_eq!(unique_index_name, "idx_commits_repo_path_hash");

        drop(database);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inserts_one_commit_and_queries_today() {
        let database = Database::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let committed_at = local_datetime_to_utc(today, 12, 0);
        let commit = build_commit("abc1234", "fix: resolve login bug", committed_at);

        database.insert_commit(&commit).unwrap();

        let commits = database.query_date(today).unwrap();

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].hash, "abc1234");
        assert_eq!(commits[0].message, "fix: resolve login bug");
        assert_eq!(commits[0].repo_name, "my-app");
        assert!(commits[0].id.is_some());
    }

    #[test]
    fn inserts_commits_across_days_and_queries_date_range() {
        let database = Database::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let yesterday = today - Duration::days(1);
        let committed_today = local_datetime_to_utc(today, 12, 0);
        let committed_yesterday = local_datetime_to_utc(yesterday, 12, 0);
        let today_commit = build_commit("aaa1111", "today's commit", committed_today);
        let yesterday_commit = build_commit("bbb2222", "yesterday's commit", committed_yesterday);

        database.insert_commit(&today_commit).unwrap();
        database.insert_commit(&yesterday_commit).unwrap();

        let today_commits = database.query_date(today).unwrap();
        let range_commits = database.query_date_range(yesterday, today).unwrap();

        assert_eq!(today_commits.len(), 1);
        assert_eq!(today_commits[0].hash, "aaa1111");
        assert_eq!(range_commits.len(), 2);
        assert_eq!(
            range_commits
                .iter()
                .map(|commit| commit.hash.as_str())
                .collect::<Vec<_>>(),
            vec!["bbb2222", "aaa1111"]
        );
    }

    #[test]
    fn duplicate_insertions_do_not_create_duplicate_rows() {
        let database = Database::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let commit = build_commit(
            "dup1234",
            "duplicate-safe commit",
            local_datetime_to_utc(today, 12, 0),
        );

        database.insert_commit(&commit).unwrap();
        database.insert_commit(&commit).unwrap();

        let commits = database.query_date(today).unwrap();

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].hash, "dup1234");
    }

    #[test]
    fn insert_commit_stores_author_email_and_query_returns_it() {
        let database = Database::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let committed_at = local_datetime_to_utc(today, 12, 0);
        let mut commit = build_commit("abc1234", "test", committed_at);
        commit.author_email = Some("me@example.com".to_string());
        commit.files_changed = 0;
        commit.insertions = 0;
        commit.deletions = 0;

        database.insert_commit(&commit).unwrap();

        let commits = database.query_date(today).unwrap();

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].author_email, Some("me@example.com".to_string()));
    }

    #[test]
    fn migration_adds_author_email_column_when_missing() {
        const OLD_SCHEMA: &str = "
            CREATE TABLE commits (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                hash TEXT NOT NULL,
                message TEXT NOT NULL,
                repo_path TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                branch TEXT NOT NULL,
                files_changed INTEGER NOT NULL DEFAULT 0,
                insertions INTEGER NOT NULL DEFAULT 0,
                deletions INTEGER NOT NULL DEFAULT 0,
                committed_at TEXT NOT NULL
            );
        ";
        let path = unique_temp_path("diddo-db-migration").join("diddo.sqlite3");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(OLD_SCHEMA).unwrap();
        }
        let database = Database::open(&path).unwrap();
        let columns = database.commit_table_column_names().unwrap();
        assert!(
            columns.contains(&"author_email".to_string()),
            "expected author_email column, got: {:?}",
            columns
        );
        fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn reinserting_same_commit_updates_incomplete_metadata() {
        let database = Database::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let yesterday = today - Duration::days(1);
        let original_committed_at = local_datetime_to_utc(yesterday, 12, 0);
        let repaired_committed_at = local_datetime_to_utc(today, 9, 30);
        let mut incomplete_commit = build_commit("abc1234", "record commit", original_committed_at);
        incomplete_commit.files_changed = 0;
        incomplete_commit.insertions = 0;
        incomplete_commit.deletions = 0;

        let mut repaired_commit = incomplete_commit.clone();
        repaired_commit.branch = "detached".to_string();
        repaired_commit.files_changed = 4;
        repaired_commit.insertions = 12;
        repaired_commit.deletions = 3;
        repaired_commit.committed_at = repaired_committed_at;

        database.insert_commit(&incomplete_commit).unwrap();
        database.insert_commit(&repaired_commit).unwrap();

        let yesterday_commits = database.query_date(yesterday).unwrap();
        let today_commits = database.query_date(today).unwrap();

        assert!(yesterday_commits.is_empty());
        assert_eq!(today_commits.len(), 1);
        assert_eq!(today_commits[0].branch, "detached");
        assert_eq!(today_commits[0].files_changed, 4);
        assert_eq!(today_commits[0].insertions, 12);
        assert_eq!(today_commits[0].deletions, 3);
        assert_eq!(today_commits[0].committed_at, repaired_committed_at);
    }

    #[test]
    fn rejects_invalid_date_ranges() {
        let database = Database::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let yesterday = today - Duration::days(1);

        let error = database.query_date_range(today, yesterday).unwrap_err();

        assert!(matches!(error, rusqlite::Error::InvalidQuery));
    }

    #[test]
    fn cache_round_trip_stores_and_retrieves_summary() {
        let database = Database::open_in_memory().unwrap();
        let key = "abc123def456";
        let summary = "Today I fixed the login bug and refactored the API.";

        assert!(database.get_cached_summary(key).unwrap().is_none());
        database.set_cached_summary(key, summary).unwrap();
        assert_eq!(
            database.get_cached_summary(key).unwrap().as_deref(),
            Some(summary)
        );
        database.set_cached_summary(key, "Updated summary").unwrap();
        assert_eq!(
            database.get_cached_summary(key).unwrap().as_deref(),
            Some("Updated summary")
        );
    }

    #[test]
    fn commit_count_returns_zero_for_empty_database() {
        let database = Database::open_in_memory().unwrap();

        assert_eq!(database.commit_count().unwrap(), 0);
    }

    #[test]
    fn commit_count_returns_number_of_inserted_commits() {
        let database = Database::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let committed_at = local_datetime_to_utc(today, 12, 0);

        database
            .insert_commit(&build_commit("aaa1111", "first", committed_at))
            .unwrap();
        database
            .insert_commit(&build_commit("bbb2222", "second", committed_at))
            .unwrap();

        assert_eq!(database.commit_count().unwrap(), 2);
    }

    #[test]
    fn oldest_commit_date_returns_none_for_empty_database() {
        let database = Database::open_in_memory().unwrap();

        assert_eq!(database.oldest_commit_date().unwrap(), None);
    }

    #[test]
    fn oldest_commit_date_returns_earliest_committed_at() {
        let database = Database::open_in_memory().unwrap();
        let today = Local::now().date_naive();
        let yesterday = today - Duration::days(1);
        let earlier = local_datetime_to_utc(yesterday, 8, 0);
        let later = local_datetime_to_utc(today, 14, 0);

        database
            .insert_commit(&build_commit("aaa1111", "older", earlier))
            .unwrap();
        database
            .insert_commit(&build_commit("bbb2222", "newer", later))
            .unwrap();

        let oldest = database.oldest_commit_date().unwrap().unwrap();
        assert_eq!(oldest, earlier.to_rfc3339());
    }

    #[test]
    fn query_datetime_range_returns_commits_within_exact_bounds() {
        let database = Database::open_in_memory().unwrap();
        let now = Utc::now();
        let hours_ago_12 = now - Duration::hours(12);
        let hours_ago_25 = now - Duration::hours(25);

        let inside = build_commit("aaa1111", "inside range", hours_ago_12);
        let outside = build_commit("bbb2222", "outside range", hours_ago_25);

        database.insert_commit(&inside).unwrap();
        database.insert_commit(&outside).unwrap();

        let commits = database
            .query_datetime_range(now - Duration::hours(24), now)
            .unwrap();

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].hash, "aaa1111");
    }

    #[test]
    fn query_datetime_range_returns_empty_when_no_commits_in_range() {
        let database = Database::open_in_memory().unwrap();
        let now = Utc::now();
        let hours_ago_48 = now - Duration::hours(48);

        let commit = build_commit("aaa1111", "old commit", hours_ago_48);
        database.insert_commit(&commit).unwrap();

        let commits = database
            .query_datetime_range(now - Duration::hours(24), now)
            .unwrap();

        assert!(commits.is_empty());
    }

    #[test]
    fn query_datetime_range_returns_multiple_commits_sorted_by_repo_then_time() {
        let database = Database::open_in_memory().unwrap();
        let now = Utc::now();
        let hours_ago_6 = now - Duration::hours(6);
        let hours_ago_3 = now - Duration::hours(3);

        let mut commit_a = build_commit("aaa1111", "earlier", hours_ago_6);
        commit_a.repo_name = "z-repo".to_string();
        let mut commit_b = build_commit("bbb2222", "later", hours_ago_3);
        commit_b.repo_name = "a-repo".to_string();

        database.insert_commit(&commit_a).unwrap();
        database.insert_commit(&commit_b).unwrap();

        let commits = database
            .query_datetime_range(now - Duration::hours(24), now)
            .unwrap();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].repo_name, "a-repo");
        assert_eq!(commits[1].repo_name, "z-repo");
    }

    #[test]
    fn local_day_bounds_convert_to_utc_range() {
        let timezone = FixedOffset::east_opt(2 * 60 * 60).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();

        let (start, end) = date_range_bounds_in_timezone(date, date, &timezone).unwrap();

        assert_eq!(start, "2026-03-09T22:00:00+00:00");
        assert_eq!(end, "2026-03-10T22:00:00+00:00");
    }

    #[test]
    fn schema_version_is_stamped_after_open() {
        let database = Database::open_in_memory().unwrap();

        let version: i64 = database
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();

        assert_eq!(version, 1);
    }

    #[test]
    fn open_upgrades_version_zero_database() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(super::SCHEMA).unwrap();
        let pre_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(pre_version, 0);

        let database = Database::initialize(connection).unwrap();

        let version: i64 = database
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn open_refuses_newer_schema_version() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.pragma_update(None, "user_version", 99).unwrap();

        let error = match Database::initialize(connection) {
            Err(error) => error,
            Ok(_) => panic!("expected initialize to reject a newer schema version"),
        };

        assert!(
            error.to_string().contains("newer than this diddo build"),
            "unexpected error message: {error}"
        );
    }

    #[test]
    fn nonexistent_local_midnight_falls_forward_to_first_resolvable_hour() {
        use chrono::LocalResult;

        let resolved = Utc.with_ymd_and_hms(2026, 3, 8, 2, 0, 0).unwrap();

        let result = super::first_resolvable_local_hour(1..=3, |hour| {
            if hour == 2 {
                LocalResult::Single(resolved)
            } else {
                LocalResult::None
            }
        });

        assert_eq!(result, Some(resolved));
    }

    #[test]
    fn first_resolvable_local_hour_returns_none_when_no_hour_resolves() {
        let result = super::first_resolvable_local_hour(1..=3, |_| chrono::LocalResult::None);

        assert_eq!(result, None);
    }

    #[test]
    fn busy_timeout_is_set() {
        let database = Database::open_in_memory().unwrap();

        let timeout_ms: i64 = database
            .connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();

        assert_eq!(timeout_ms, 5000);
    }

    #[test]
    fn wal_mode_enabled_for_file_database() {
        let dir = unique_temp_path("diddo-db-wal");
        let path = dir.join("diddo.sqlite3");

        let database = Database::open(&path).unwrap();

        let journal_mode: String = database
            .connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        let synchronous: i64 = database
            .connection
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .unwrap();

        assert_eq!(journal_mode.to_lowercase(), "wal");
        assert_eq!(synchronous, 1);

        drop(database);
        // Removing the containing temp dir also removes the -wal/-shm sidecars.
        fs::remove_dir_all(&dir).ok();
    }

    fn schema_object_name(database: &Database, object_type: &str, object_name: &str) -> String {
        database
            .connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = ?1 AND name = ?2",
                [object_type, object_name],
                |row: &Row<'_>| row.get(0),
            )
            .unwrap()
    }

    fn schema_object_sql(database: &Database, object_type: &str, object_name: &str) -> String {
        database
            .connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = ?1 AND name = ?2",
                [object_type, object_name],
                |row: &Row<'_>| row.get(0),
            )
            .unwrap()
    }

    fn commit_columns(database: &Database) -> Vec<(String, String, bool, Option<String>, bool)> {
        let mut statement = database
            .connection
            .prepare("PRAGMA table_info(commits)")
            .unwrap();
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get(1)?,
                    row.get(2)?,
                    row.get::<_, i64>(3)? != 0,
                    row.get(4)?,
                    row.get::<_, i64>(5)? != 0,
                ))
            })
            .unwrap();

        rows.map(|row| row.unwrap()).collect()
    }

    fn normalize_sql(sql: &str) -> String {
        sql.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn build_commit(hash: &str, message: &str, committed_at: chrono::DateTime<Utc>) -> Commit {
        Commit {
            id: None,
            hash: hash.to_string(),
            message: message.to_string(),
            repo_path: "/home/user/projects/my-app".to_string(),
            repo_name: "my-app".to_string(),
            branch: "main".to_string(),
            files_changed: 3,
            insertions: 25,
            deletions: 10,
            committed_at,
            author_email: None,
        }
    }

    fn local_datetime_to_utc(date: NaiveDate, hour: u32, minute: u32) -> chrono::DateTime<Utc> {
        let naive = date.and_hms_opt(hour, minute, 0).unwrap();
        Local
            .from_local_datetime(&naive)
            .earliest()
            .unwrap()
            .with_timezone(&Utc)
    }

    fn unique_temp_path(prefix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
