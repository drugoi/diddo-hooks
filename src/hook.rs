use std::{io, path::Path, process::Command};

use chrono::{DateTime, Utc};

use crate::db::{Commit, Database};

pub fn run(_db: &Database) -> io::Result<()> {
    run_with(_db, run_git_command, read_diff_stats)
}

fn run_with<F, G>(db: &Database, mut run_git: F, mut read_diff_stats: G) -> io::Result<()>
where
    F: FnMut(&[&str]) -> io::Result<String>,
    G: FnMut() -> io::Result<String>,
{
    let commit = build_commit(&mut run_git, &mut read_diff_stats)?;

    db.insert_commit(&commit).map_err(io::Error::other)
}

fn build_commit<F, G>(run_git: &mut F, read_diff_stats: &mut G) -> io::Result<Commit>
where
    F: FnMut(&[&str]) -> io::Result<String>,
    G: FnMut() -> io::Result<String>,
{
    let hash = trim_git_output(run_git(&["rev-parse", "--short", "HEAD"])?);
    let message = trim_git_output(run_git(&["log", "-1", "--format=%B"])?);
    let committed_at =
        parse_git_timestamp(&trim_git_output(run_git(&["log", "-1", "--format=%cI"])?))?;
    let repo_path = trim_git_output(run_git(&["rev-parse", "--show-toplevel"])?);
    let branch = normalize_branch_name(&trim_git_output(run_git(&[
        "rev-parse",
        "--abbrev-ref",
        "HEAD",
    ])?));
    let repo_name = repo_name_from_path(&repo_path);
    let (files_changed, insertions, deletions) = read_diff_stats()
        .ok()
        .and_then(|output| parse_diff_stats(&output))
        .unwrap_or((0, 0, 0));

    let author_email = run_git(&["config", "user.email"])
        .ok()
        .map(trim_git_output)
        .filter(|s| !s.is_empty());

    Ok(Commit {
        id: None,
        hash,
        message,
        repo_path,
        repo_name,
        branch,
        files_changed,
        insertions,
        deletions,
        committed_at,
        author_email,
    })
}

fn run_git_command(args: &[&str]) -> io::Result<String> {
    let output = Command::new("git").args(args).output()?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let command = format!("git {}", args.join(" "));
    let message = if stderr.is_empty() {
        format!("{command} failed")
    } else {
        format!("{command} failed: {stderr}")
    };

    Err(io::Error::other(message))
}

fn read_diff_stats() -> io::Result<String> {
    run_git_command(&["show", "--shortstat", "--format=", "HEAD"])
}

fn parse_diff_stats(output: &str) -> Option<(i64, i64, i64)> {
    let summary = output
        .lines()
        .rev()
        .find(|line| line.contains("file changed") || line.contains("files changed"))?;

    let mut files_changed = 0;
    let mut insertions = 0;
    let mut deletions = 0;

    for part in summary.split(',').map(str::trim) {
        let value = part
            .split_whitespace()
            .next()
            .and_then(|number| number.parse::<i64>().ok())
            .unwrap_or(0);

        if part.contains("file changed") || part.contains("files changed") {
            files_changed = value;
        } else if part.contains("insertion") {
            insertions = value;
        } else if part.contains("deletion") {
            deletions = value;
        }
    }

    Some((files_changed, insertions, deletions))
}

fn trim_git_output(output: String) -> String {
    output.trim_end_matches(['\r', '\n']).trim().to_string()
}

fn parse_git_timestamp(timestamp: &str) -> io::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(timestamp)
        .map(|value| value.with_timezone(&Utc))
        .map_err(io::Error::other)
}

fn repo_name_from_path(repo_path: &str) -> String {
    Path::new(repo_path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

fn normalize_branch_name(branch: &str) -> String {
    if branch == "HEAD" {
        return "detached".to_string();
    }

    branch.to_string()
}

#[cfg(test)]
mod tests {
    use std::io;

    use chrono::{DateTime, Local, TimeZone, Utc};

    use super::{build_commit, normalize_branch_name, parse_git_timestamp, run_with};
    use crate::db::Database;

    #[test]
    fn stores_author_email_from_git_config() {
        let database = Database::open_in_memory().unwrap();
        let committed_at = Utc.with_ymd_and_hms(2026, 3, 10, 12, 0, 0).unwrap();

        run_with(
            &database,
            |args| match args {
                ["rev-parse", "--short", "HEAD"] => Ok("abc1234\n".to_string()),
                ["log", "-1", "--format=%B"] => Ok("feat: add hook storage\n".to_string()),
                ["log", "-1", "--format=%cI"] => Ok("2026-03-10T12:00:00+00:00\n".to_string()),
                ["rev-parse", "--show-toplevel"] => {
                    Ok("/Users/example/projects/diddo\n".to_string())
                }
                ["rev-parse", "--abbrev-ref", "HEAD"] => Ok("feature/diddo\n".to_string()),
                ["config", "user.email"] => Ok("work@company.com\n".to_string()),
                _ => Err(io::Error::other("unexpected git arguments")),
            },
            || Ok(" 2 files changed, 10 insertions(+), 3 deletions(-)\n".to_string()),
        )
        .unwrap();

        let commits = database
            .query_date(committed_at.with_timezone(&Local).date_naive())
            .unwrap();

        assert_eq!(commits.len(), 1);
        assert_eq!(
            commits[0].author_email,
            Some("work@company.com".to_string())
        );
    }

    #[test]
    fn stores_git_metadata_in_database() {
        let database = Database::open_in_memory().unwrap();
        let committed_at = Utc.with_ymd_and_hms(2026, 3, 10, 12, 0, 0).unwrap();

        run_with(
            &database,
            |args| match args {
                ["rev-parse", "--short", "HEAD"] => Ok("abc1234\n".to_string()),
                ["log", "-1", "--format=%B"] => Ok("feat: add hook storage\n".to_string()),
                ["log", "-1", "--format=%cI"] => Ok("2026-03-10T12:00:00+00:00\n".to_string()),
                ["rev-parse", "--show-toplevel"] => {
                    Ok("/Users/example/projects/diddo\n".to_string())
                }
                ["rev-parse", "--abbrev-ref", "HEAD"] => Ok("feature/diddo\n".to_string()),
                ["config", "user.email"] => Ok("work@company.com\n".to_string()),
                _ => Err(io::Error::other("unexpected git arguments")),
            },
            || Ok(" 2 files changed, 10 insertions(+), 3 deletions(-)\n".to_string()),
        )
        .unwrap();

        let commits = database
            .query_date(committed_at.with_timezone(&Local).date_naive())
            .unwrap();

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].hash, "abc1234");
        assert_eq!(commits[0].message, "feat: add hook storage");
        assert_eq!(commits[0].repo_path, "/Users/example/projects/diddo");
        assert_eq!(commits[0].repo_name, "diddo");
        assert_eq!(commits[0].branch, "feature/diddo");
        assert_eq!(commits[0].files_changed, 2);
        assert_eq!(commits[0].insertions, 10);
        assert_eq!(commits[0].deletions, 3);
        assert_eq!(commits[0].committed_at, committed_at);
        assert_eq!(
            commits[0].author_email,
            Some("work@company.com".to_string())
        );
    }

    #[test]
    fn author_email_is_none_when_config_fails() {
        let mut run_git = |args: &[&str]| match args {
            ["rev-parse", "--short", "HEAD"] => Ok("abc1234\n".to_string()),
            ["log", "-1", "--format=%B"] => Ok("feat: add hook storage\n".to_string()),
            ["log", "-1", "--format=%cI"] => Ok("2026-03-10T12:00:00+00:00\n".to_string()),
            ["rev-parse", "--show-toplevel"] => Ok("/Users/example/projects/diddo\n".to_string()),
            ["rev-parse", "--abbrev-ref", "HEAD"] => Ok("feature/diddo\n".to_string()),
            ["config", "user.email"] => Err(io::Error::other("no config")),
            _ => Err(io::Error::other("unexpected git arguments")),
        };
        let mut read_diff_stats =
            || Ok(" 2 files changed, 10 insertions(+), 3 deletions(-)\n".to_string());

        let commit = build_commit(&mut run_git, &mut read_diff_stats).unwrap();

        assert_eq!(commit.author_email, None);
    }

    #[test]
    fn defaults_diff_stats_to_zero_when_extraction_fails() {
        let mut run_git = |args: &[&str]| match args {
            ["rev-parse", "--short", "HEAD"] => Ok("abc1234\n".to_string()),
            ["log", "-1", "--format=%B"] => Ok("feat: add hook storage\n".to_string()),
            ["log", "-1", "--format=%cI"] => Ok("2026-03-10T12:00:00+00:00\n".to_string()),
            ["rev-parse", "--show-toplevel"] => Ok("/Users/example/projects/diddo\n".to_string()),
            ["rev-parse", "--abbrev-ref", "HEAD"] => Ok("feature/diddo\n".to_string()),
            ["config", "user.email"] => Ok("work@company.com\n".to_string()),
            _ => Err(io::Error::other("unexpected git arguments")),
        };
        let mut read_diff_stats = || Err(io::Error::other("diff stats unavailable"));

        let commit = build_commit(&mut run_git, &mut read_diff_stats).unwrap();

        assert_eq!(commit.files_changed, 0);
        assert_eq!(commit.insertions, 0);
        assert_eq!(commit.deletions, 0);
    }

    #[test]
    fn normalizes_detached_head_branch_name() {
        assert_eq!(normalize_branch_name("HEAD"), "detached");
        assert_eq!(normalize_branch_name("feature/diddo"), "feature/diddo");
    }

    #[test]
    fn uses_git_commit_timestamp_for_committed_at() {
        let mut run_git = |args: &[&str]| match args {
            ["rev-parse", "--short", "HEAD"] => Ok("abc1234\n".to_string()),
            ["log", "-1", "--format=%B"] => Ok("feat: add hook storage\n".to_string()),
            ["log", "-1", "--format=%cI"] => Ok("2026-03-09T23:45:00-05:00\n".to_string()),
            ["rev-parse", "--show-toplevel"] => Ok("/Users/example/projects/diddo\n".to_string()),
            ["rev-parse", "--abbrev-ref", "HEAD"] => Ok("feature/diddo\n".to_string()),
            ["config", "user.email"] => Ok("dev@example.com\n".to_string()),
            _ => Err(io::Error::other("unexpected git arguments")),
        };
        let mut read_diff_stats =
            || Ok(" 2 files changed, 10 insertions(+), 3 deletions(-)\n".to_string());

        let commit = build_commit(&mut run_git, &mut read_diff_stats).unwrap();

        assert_eq!(
            commit.committed_at,
            DateTime::parse_from_rfc3339("2026-03-09T23:45:00-05:00")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn parses_git_iso_timestamp_into_utc() {
        assert_eq!(
            parse_git_timestamp("2026-03-09T23:45:00-05:00").unwrap(),
            DateTime::parse_from_rfc3339("2026-03-09T23:45:00-05:00")
                .unwrap()
                .with_timezone(&Utc)
        );
    }
}
