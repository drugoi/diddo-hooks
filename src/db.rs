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
            fs::create_dir_all(parent)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        }

        let connection = Connection::open(path)?;
        Self::initialize(connection)
    }

    pub fn open_in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        Self::initialize(connection)
    }

    fn initialize(connection: Connection) -> Result<Self> {
        connection.execute_batch(SCHEMA)?;
        run_author_email_migration(&connection)?;

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
        let mut stmt = self.connection.prepare(
            "SELECT summary FROM ai_summary_cache WHERE cache_key = ?1",
        )?;
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

    let datetime = match timezone.from_local_datetime(&local_midnight) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(first, _) => first,
        LocalResult::None => return Err(rusqlite::Error::InvalidQuery),
    };

    Ok(datetime.with_timezone(&Utc))
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
                ("author_email".to_string(), "TEXT".to_string(), false, None, false),
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
        assert_eq!(
            commits[0].author_email,
            Some("me@example.com".to_string())
        );
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
    fn local_day_bounds_convert_to_utc_range() {
        let timezone = FixedOffset::east_opt(2 * 60 * 60).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();

        let (start, end) = date_range_bounds_in_timezone(date, date, &timezone).unwrap();

        assert_eq!(start, "2026-03-09T22:00:00+00:00");
        assert_eq!(end, "2026-03-10T22:00:00+00:00");
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
