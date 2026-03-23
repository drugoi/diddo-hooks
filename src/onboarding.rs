#![cfg_attr(
    not(test),
    allow(dead_code)
)] // Core helpers are covered by unit tests; `run()` wires them in later tasks.

use std::error::Error;
use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};

use crate::config;
use crate::db::{Commit, Database};

/// Commit metadata scanned from `git log` before conversion to [`Commit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedCommit {
    pub hash: String,
    pub message: String,
    pub author_name: String,
    pub author_email: Option<String>,
    pub committed_at: DateTime<Utc>,
}

/// Author identity the user selected for import (email preferred; name when email is absent on the commit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityCandidate {
    pub name: Option<String>,
    pub email: Option<String>,
}

/// Counts for one import run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportOutcome {
    pub scanned: usize,
    pub matched: usize,
    pub inserted: usize,
    pub skipped_duplicates: usize,
}

/// Full onboarding flow (interactive UI, git scan) is implemented incrementally.
pub fn run(
    _database: &Database,
    _config_path: &Path,
    _config: config::AppConfig,
) -> Result<(), Box<dyn Error>> {
    Ok(())
}

/// `git log` line format: hash, subject, author name, author email, commit date (RFC3339), separated by ASCII unit separator.
#[allow(dead_code)] // Used when wiring `git log`; must stay aligned with [`parse_git_log_output`].
pub const GIT_LOG_FORMAT: &str = "%H%x1f%s%x1f%an%x1f%ae%x1f%cI";

/// Parse output from `git log` using [`GIT_LOG_FORMAT`] (one record per line).
pub fn parse_git_log_output(output: &str) -> Result<Vec<ScannedCommit>, String> {
    let mut commits = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split('\x1f').collect();
        if parts.len() != 5 {
            return Err(format!(
                "expected 5 fields per log line, got {}",
                parts.len()
            ));
        }

        let hash = parts[0].trim().to_string();
        let message = parts[1].to_string();
        let author_name = parts[2].trim().to_string();
        let author_email = {
            let e = parts[3].trim();
            if e.is_empty() {
                None
            } else {
                Some(e.to_string())
            }
        };

        let committed_at = DateTime::parse_from_rfc3339(parts[4].trim())
            .map_err(|e| e.to_string())?
            .with_timezone(&Utc);

        commits.push(ScannedCommit {
            hash,
            message,
            author_name,
            author_email,
            committed_at,
        });
    }

    Ok(commits)
}

/// Unique author identities from scanned commits, sorted with email-backed rows first, then by email and name.
pub fn detect_identities(commits: &[ScannedCommit]) -> Vec<IdentityCandidate> {
    use std::collections::HashSet;

    let mut seen: HashSet<(Option<String>, Option<String>)> = HashSet::new();
    let mut out = Vec::new();

    for c in commits {
        let email = c
            .author_email
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let name = if c.author_name.trim().is_empty() {
            None
        } else {
            Some(c.author_name.trim().to_string())
        };

        if email.is_none() && name.is_none() {
            continue;
        }

        let key = (email.clone(), name.clone());
        if seen.insert(key) {
            out.push(IdentityCandidate { name, email });
        }
    }

    out.sort_by(|a, b| match (&a.email, &b.email) {
        (Some(ea), Some(eb)) => ea
            .cmp(eb)
            .then_with(|| cmp_opt_str(&a.name, &b.name)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => cmp_opt_str(&a.name, &b.name),
    });

    out
}

fn cmp_opt_str(a: &Option<String>, b: &Option<String>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// Keep commits on or after `cutoff` (local date vs UTC timestamps) whose author matches `selected`.
pub fn filter_importable_commits(
    commits: &[ScannedCommit],
    cutoff: NaiveDate,
    selected: &[IdentityCandidate],
) -> Vec<ScannedCommit> {
    commits
        .iter()
        .filter(|c| {
            c.committed_at.date_naive() >= cutoff && identity_matches(c, selected)
        })
        .cloned()
        .collect()
}

fn identity_matches(commit: &ScannedCommit, selected: &[IdentityCandidate]) -> bool {
    selected.iter().any(|id| {
        if let Some(ref ce) = commit.author_email {
            id.email
                .as_ref()
                .is_some_and(|se| ce.eq_ignore_ascii_case(se))
        } else {
            match (&commit.author_name, &id.name) {
                (cn, Some(sn)) => cn == sn,
                _ => false,
            }
        }
    })
}

pub fn to_db_commit(
    scanned: &ScannedCommit,
    repo_path: &str,
    repo_name: &str,
    branch: &str,
) -> Commit {
    Commit {
        id: None,
        hash: scanned.hash.clone(),
        message: scanned.message.clone(),
        repo_path: repo_path.to_string(),
        repo_name: repo_name.to_string(),
        branch: branch.to_string(),
        files_changed: 0,
        insertions: 0,
        deletions: 0,
        committed_at: scanned.committed_at,
        author_email: scanned.author_email.clone(),
    }
}

/// Insert scanned commits into the database. Duplicate `(repo_path, hash)` rows are updated in place, not double-counted.
pub fn import_commits(
    database: &Database,
    repo_path: &str,
    repo_name: &str,
    branch: &str,
    commits: &[ScannedCommit],
) -> Result<ImportOutcome, rusqlite::Error> {
    let scanned = commits.len();
    let matched = commits.len();
    let count_before = database.commit_count()?;

    for c in commits {
        let row = to_db_commit(c, repo_path, repo_name, branch);
        database.insert_commit(&row)?;
    }

    let count_after = database.commit_count()?;
    let inserted = (count_after - count_before) as usize;
    let skipped_duplicates = matched.saturating_sub(inserted);

    Ok(ImportOutcome {
        scanned,
        matched,
        inserted,
        skipped_duplicates,
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::db::Database;

    fn sample_commit(
        hash: &str,
        message: &str,
        author_name: &str,
        author_email: Option<&str>,
        committed_at: DateTime<Utc>,
    ) -> ScannedCommit {
        ScannedCommit {
            hash: hash.to_string(),
            message: message.to_string(),
            author_name: author_name.to_string(),
            author_email: author_email.map(str::to_string),
            committed_at,
        }
    }

    #[test]
    fn filters_commits_on_or_after_cutoff_date() {
        let cutoff = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
        let selected = vec![IdentityCandidate {
            name: None,
            email: Some("me@example.com".to_string()),
        }];
        let commits = vec![
            sample_commit(
                "aaa1111",
                "old",
                "Me",
                Some("me@example.com"),
                Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap(),
            ),
            sample_commit(
                "bbb2222",
                "new",
                "Me",
                Some("me@example.com"),
                Utc.with_ymd_and_hms(2026, 1, 3, 12, 0, 0).unwrap(),
            ),
        ];

        let imported = filter_importable_commits(&commits, cutoff, &selected);

        assert_eq!(
            imported.iter().map(|c| c.hash.as_str()).collect::<Vec<_>>(),
            vec!["bbb2222"]
        );
    }

    #[test]
    fn imports_only_selected_identities() {
        let cutoff = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let selected = vec![IdentityCandidate {
            name: None,
            email: Some("me@example.com".to_string()),
        }];
        let commits = vec![
            sample_commit(
                "aaa1111",
                "a",
                "A",
                Some("other@example.com"),
                Utc.with_ymd_and_hms(2026, 1, 5, 12, 0, 0).unwrap(),
            ),
            sample_commit(
                "bbb2222",
                "b",
                "Me",
                Some("me@example.com"),
                Utc.with_ymd_and_hms(2026, 1, 5, 12, 0, 0).unwrap(),
            ),
        ];

        let imported = filter_importable_commits(&commits, cutoff, &selected);

        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].author_email.as_deref(), Some("me@example.com"));
    }

    #[test]
    fn import_is_idempotent_when_re_run() {
        let database = Database::open_in_memory().unwrap();
        let repo_path = "/tmp/repo";
        let repo_name = "repo";
        let branch = "main";
        let filtered = vec![sample_commit(
            "abc1234",
            "msg",
            "Me",
            Some("me@example.com"),
            Utc.with_ymd_and_hms(2026, 2, 1, 12, 0, 0).unwrap(),
        )];

        import_commits(&database, repo_path, repo_name, branch, &filtered).unwrap();
        import_commits(&database, repo_path, repo_name, branch, &filtered).unwrap();

        assert_eq!(database.commit_count().unwrap(), 1);
    }

    fn sample_log_two_commits() -> &'static str {
        "abc1234\x1ffirst\x1fAuthor One\x1fme@example.com\x1f2026-01-01T12:00:00+00:00\ndef5678\x1fsecond\x1fAuthor Two\x1fother@example.com\x1f2026-01-02T12:00:00+00:00"
    }

    #[test]
    fn parse_git_log_output_builds_scanned_commits() {
        let commits = parse_git_log_output(sample_log_two_commits()).unwrap();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].hash, "abc1234");
    }

    #[test]
    fn detected_identities_include_unique_name_email_pairs() {
        let commits = parse_git_log_output(sample_log_two_commits()).unwrap();
        let identities = detect_identities(&commits);

        assert_eq!(identities.len(), 2);
        assert!(identities
            .iter()
            .any(|i| i.email.as_deref() == Some("me@example.com")));
    }
}
