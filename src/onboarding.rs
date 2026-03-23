use std::error::Error;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::Command;

use chrono::{DateTime, NaiveDate, Utc};

use crate::config::{self, IdentityAlias};
use crate::db::{Commit, Database};
use crate::parse_supported_date;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedCommit {
    pub hash: String,
    pub message: String,
    pub author_name: String,
    pub author_email: Option<String>,
    pub committed_at: DateTime<Utc>,
    pub committed_local_date: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityCandidate {
    pub name: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitIdentity {
    pub name: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportOutcome {
    pub scanned: usize,
    pub matched: usize,
    pub inserted: usize,
    pub skipped_duplicates: usize,
}

pub fn run(
    database: &Database,
    config_path: &Path,
    config: config::AppConfig,
) -> Result<(), Box<dyn Error>> {
    let stdin = io::stdin();
    let mut stdin_lock = stdin.lock();
    let mut stdout = io::stdout();

    let outcome = run_with(
        database,
        config_path,
        config,
        &mut || {
            let mut line = String::new();
            stdin_lock.read_line(&mut line)?;
            Ok(line)
        },
        &mut |s: &str| {
            stdout.write_all(s.as_bytes())?;
            stdout.flush()?;
            Ok(())
        },
        run_git_command,
    )?;

    writeln!(
        io::stdout(),
        "Onboarding finished: {} commit(s) in date range, {} matched selected identities, {} inserted, {} skipped as duplicates.",
        outcome.scanned,
        outcome.matched,
        outcome.inserted,
        outcome.skipped_duplicates
    )?;

    Ok(())
}

pub fn run_with<R, W, G>(
    database: &Database,
    config_path: &Path,
    config: config::AppConfig,
    read_line: &mut R,
    write_line: &mut W,
    mut run_git: G,
) -> Result<ImportOutcome, Box<dyn Error>>
where
    R: FnMut() -> io::Result<String>,
    W: FnMut(&str) -> io::Result<()>,
    G: FnMut(&[&str]) -> io::Result<String>,
{
    let repo_path = trim_git_output(run_git(&["rev-parse", "--show-toplevel"])?);
    if repo_path.is_empty() {
        return Err("not a git repository (git rev-parse --show-toplevel failed)".into());
    }

    let branch = normalize_branch_name(&trim_git_output(run_git(&[
        "rev-parse",
        "--abbrev-ref",
        "HEAD",
    ])?));
    let repo_name = repo_name_from_path(&repo_path);

    let format_arg = format!("--format={}", GIT_LOG_FORMAT);
    let log_out = run_git(&["log", "-z", &format_arg])?;
    let all_commits = parse_git_log_output(&log_out).map_err(Box::<dyn Error>::from)?;

    if all_commits.is_empty() {
        write_line("No commits found in this repository — nothing to import.\n")?;
        return Ok(ImportOutcome::default());
    }

    write_line(&format!(
        "Import commits on or after which date? ({}) ",
        RANGE_DATE_HINT
    ))?;
    let cutoff_line = read_line()?;
    let cutoff = parse_supported_date(cutoff_line.trim())
        .map_err(|e| Box::<dyn Error>::from(format!("invalid cutoff date: {e}")))?;

    let scanned = all_commits
        .iter()
        .filter(|c| c.committed_local_date >= cutoff)
        .count();

    if scanned == 0 {
        write_line(&format!(
            "No commits on or after {} — nothing to import.\n",
            cutoff
        ))?;
        return Ok(ImportOutcome {
            scanned: 0,
            matched: 0,
            inserted: 0,
            skipped_duplicates: 0,
        });
    }

    let detected = detect_identities(&all_commits);
    let current = read_git_identity(&mut run_git)?;
    let preselected =
        build_preselected_identities(&current, &config.onboarding.identity_aliases, &detected);

    if detected.is_empty() {
        write_line("No author identities found in history — nothing to import.\n")?;
        return Ok(ImportOutcome {
            scanned,
            matched: 0,
            inserted: 0,
            skipped_duplicates: 0,
        });
    }

    write_line("\nAuthor identities (from git history):\n")?;
    for (i, id) in detected.iter().enumerate() {
        let label = format_identity_line(id);
        let mark = if preselected.iter().any(|p| identity_same(p, id)) {
            " (preselected)"
        } else {
            ""
        };
        write_line(&format!("{}. {}{}\n", i + 1, label, mark))?;
    }
    write_line(
        "\nEnter numbers to import, separated by commas (e.g. 1,3). Press Enter to use preselected only.\n> ",
    )?;

    let selection_line = read_line()?;
    let selected = parse_identity_selection(selection_line.trim(), &detected, &preselected)?;

    let filtered = filter_importable_commits(&all_commits, cutoff, &selected);
    let matched = filtered.len();

    let import_outcome = import_commits(
        database, &repo_path, &repo_name, &branch, &filtered, scanned, matched,
    )?;

    if config.onboarding.save_selected_identities {
        let had_before = config.onboarding.identity_aliases.clone();

        let selected_aliases: Vec<IdentityAlias> = selected
            .iter()
            .map(|id| IdentityAlias {
                name: id.name.clone(),
                email: id.email.clone(),
            })
            .collect();

        let has_new_alias = selected_aliases.iter().any(|a| {
            !had_before
                .iter()
                .any(|h| h.name == a.name && h.email == a.email)
        });

        if has_new_alias {
            write_line("\nSave selected identities to config for next time? [y/N] ")?;
            let yn = read_line()?;
            if matches!(yn.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                let mut merged = had_before;
                for a in selected_aliases {
                    if !merged
                        .iter()
                        .any(|h| h.name == a.name && h.email == a.email)
                    {
                        merged.push(a);
                    }
                }
                config::save_onboarding_aliases(config_path, &merged)?;
                write_line("Saved identity aliases to config.\n")?;
            }
        }
    }

    Ok(import_outcome)
}

const RANGE_DATE_HINT: &str = "YYYY-MM-DD or DD.MM.YYYY";

pub const GIT_LOG_FORMAT: &str = "%H%x1f%an%x1f%ae%x1f%cI%x1f%B";

pub fn parse_git_log_output(output: &str) -> Result<Vec<ScannedCommit>, String> {
    let mut commits = Vec::new();

    for record in output.split('\0') {
        let record = record.trim();
        if record.is_empty() {
            continue;
        }

        let parts: Vec<&str> = record.split('\x1f').collect();
        if parts.len() != 5 {
            return Err(format!(
                "expected 5 fields per commit record, got {}",
                parts.len()
            ));
        }

        let hash = parts[0].trim().to_string();
        let author_name = parts[1].trim().to_string();
        let author_email = {
            let e = parts[2].trim();
            if e.is_empty() {
                None
            } else {
                Some(e.to_string())
            }
        };

        let dt = DateTime::parse_from_rfc3339(parts[3].trim()).map_err(|e| e.to_string())?;
        let committed_at = dt.with_timezone(&Utc);
        let committed_local_date = dt.naive_local().date();

        let message = parts[4].to_string();

        commits.push(ScannedCommit {
            hash,
            message,
            author_name,
            author_email,
            committed_at,
            committed_local_date,
        });
    }

    Ok(commits)
}

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
        (Some(ea), Some(eb)) => ea.cmp(eb).then_with(|| cmp_opt_str(&a.name, &b.name)),
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

pub fn build_preselected_identities(
    current: &GitIdentity,
    saved_aliases: &[IdentityAlias],
    detected: &[IdentityCandidate],
) -> Vec<IdentityCandidate> {
    use std::collections::HashSet;

    fn push_unique(
        out: &mut Vec<IdentityCandidate>,
        seen: &mut HashSet<(Option<String>, Option<String>)>,
        id: IdentityCandidate,
    ) {
        let key = (id.email.clone(), id.name.clone());
        if seen.insert(key) {
            out.push(id);
        }
    }

    let mut seen: HashSet<(Option<String>, Option<String>)> = HashSet::new();
    let mut out = Vec::new();

    if let Some(ref e) = current.email {
        let e = e.trim();
        if !e.is_empty() {
            push_unique(
                &mut out,
                &mut seen,
                IdentityCandidate {
                    name: current
                        .name
                        .as_ref()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty()),
                    email: Some(e.to_string()),
                },
            );
        }
    }

    for a in saved_aliases {
        let email = a
            .email
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let name = a
            .name
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if email.is_none() && name.is_none() {
            continue;
        }
        push_unique(&mut out, &mut seen, IdentityCandidate { name, email });
    }

    let anchor_emails: HashSet<String> = out
        .iter()
        .filter_map(|i| i.email.as_ref().map(|e| e.to_ascii_lowercase()))
        .collect();

    let detected_has_email = detected.iter().any(|d| d.email.is_some());

    for d in detected {
        if let Some(ref de) = d.email {
            if anchor_emails.iter().any(|a| a.eq_ignore_ascii_case(de)) {
                push_unique(&mut out, &mut seen, d.clone());
            }
        } else if !detected_has_email
            && let (Some(cn), Some(dn)) = (
                current
                    .name
                    .as_ref()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty()),
                d.name.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()),
            )
            && cn == dn
        {
            push_unique(&mut out, &mut seen, d.clone());
        }
    }

    out
}

pub fn filter_importable_commits(
    commits: &[ScannedCommit],
    cutoff: NaiveDate,
    selected: &[IdentityCandidate],
) -> Vec<ScannedCommit> {
    commits
        .iter()
        .filter(|c| c.committed_local_date >= cutoff && identity_matches(c, selected))
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
            match (&id.name, &commit.author_name) {
                (Some(sn), cn) => cn.trim().eq_ignore_ascii_case(sn.trim()),
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

pub fn import_commits(
    database: &Database,
    repo_path: &str,
    repo_name: &str,
    branch: &str,
    commits: &[ScannedCommit],
    scanned_total: usize,
    matched_total: usize,
) -> Result<ImportOutcome, rusqlite::Error> {
    let count_before = database.commit_count()?;

    for c in commits {
        let row = to_db_commit(c, repo_path, repo_name, branch);
        database.insert_commit(&row)?;
    }

    let count_after = database.commit_count()?;
    let inserted = (count_after - count_before) as usize;
    let skipped_duplicates = commits.len().saturating_sub(inserted);

    Ok(ImportOutcome {
        scanned: scanned_total,
        matched: matched_total,
        inserted,
        skipped_duplicates,
    })
}

fn trim_git_output(output: String) -> String {
    output.trim_end_matches(['\r', '\n']).trim().to_string()
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

fn read_git_identity<F>(run_git: &mut F) -> io::Result<GitIdentity>
where
    F: FnMut(&[&str]) -> io::Result<String>,
{
    let email = run_git(&["config", "user.email"])
        .ok()
        .map(trim_git_output)
        .filter(|s| !s.is_empty());
    let name = run_git(&["config", "user.name"])
        .ok()
        .map(trim_git_output)
        .filter(|s| !s.is_empty());
    Ok(GitIdentity { name, email })
}

fn format_identity_line(id: &IdentityCandidate) -> String {
    match (&id.name, &id.email) {
        (Some(n), Some(e)) => format!("{n} <{e}>"),
        (None, Some(e)) => e.clone(),
        (Some(n), None) => format!("{n} (no email)"),
        (None, None) => "(unknown)".to_string(),
    }
}

fn identity_same(a: &IdentityCandidate, b: &IdentityCandidate) -> bool {
    a.name == b.name && a.email == b.email
}

fn parse_identity_selection(
    line: &str,
    candidates: &[IdentityCandidate],
    preselected: &[IdentityCandidate],
) -> Result<Vec<IdentityCandidate>, String> {
    if line.is_empty() {
        return Ok(preselected.to_vec());
    }

    use std::collections::HashSet;

    let mut seen_idx: HashSet<usize> = HashSet::new();
    let mut out = Vec::new();

    for part in line.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let n: usize = part
            .parse()
            .map_err(|_| format!("invalid selection index: {part}"))?;
        let idx = n
            .checked_sub(1)
            .ok_or_else(|| "selection indices must be >= 1".to_string())?;
        let id = candidates
            .get(idx)
            .ok_or_else(|| format!("no identity at index {n}"))?;
        if seen_idx.insert(idx) {
            out.push(id.clone());
        }
    }

    if out.is_empty() {
        return Err("no identities selected".into());
    }

    Ok(out)
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

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};

    use super::*;
    use crate::config::IdentityAlias;
    use crate::db::{Commit, Database};

    const TEST_FULL_HASH: &str = "abcd1234abcd1234abcd1234abcd1234abcd1234";

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
            committed_local_date: committed_at.date_naive(),
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

        import_commits(
            &database,
            repo_path,
            repo_name,
            branch,
            &filtered,
            filtered.len(),
            filtered.len(),
        )
        .unwrap();
        import_commits(
            &database,
            repo_path,
            repo_name,
            branch,
            &filtered,
            filtered.len(),
            filtered.len(),
        )
        .unwrap();

        assert_eq!(database.commit_count().unwrap(), 1);
    }

    fn sample_log_two_commits() -> &'static str {
        "abc1234\x1fAuthor One\x1fme@example.com\x1f2026-01-01T12:00:00+00:00\x1ffirst subject\0\
         def5678\x1fAuthor Two\x1fother@example.com\x1f2026-01-02T12:00:00+00:00\x1fsecond subject\0"
    }

    #[test]
    fn parse_git_log_output_builds_scanned_commits() {
        let commits = parse_git_log_output(sample_log_two_commits()).unwrap();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].hash, "abc1234");
        assert_eq!(commits[0].message, "first subject");
    }

    #[test]
    fn detected_identities_include_unique_name_email_pairs() {
        let commits = parse_git_log_output(sample_log_two_commits()).unwrap();
        let identities = detect_identities(&commits);

        assert_eq!(identities.len(), 2);
        assert!(
            identities
                .iter()
                .any(|i| i.email.as_deref() == Some("me@example.com"))
        );
    }

    #[test]
    fn preselects_current_git_email_and_saved_aliases() {
        let current_identity = GitIdentity {
            name: Some("Me".to_string()),
            email: Some("me@example.com".to_string()),
        };
        let saved_aliases = vec![IdentityAlias {
            name: None,
            email: Some("me@old-company.com".to_string()),
        }];
        let detected = vec![IdentityCandidate {
            name: Some("Other".to_string()),
            email: Some("other@example.com".to_string()),
        }];

        let selected = build_preselected_identities(&current_identity, &saved_aliases, &detected);

        assert!(
            selected
                .iter()
                .any(|i| i.email.as_deref() == Some("me@example.com"))
        );
        assert!(
            selected
                .iter()
                .any(|i| i.email.as_deref() == Some("me@old-company.com"))
        );
    }

    #[test]
    fn name_only_matches_are_not_auto_selected_when_email_candidates_exist() {
        let current_identity = GitIdentity {
            name: Some("User".to_string()),
            email: Some("me@example.com".to_string()),
        };
        let detected = vec![
            IdentityCandidate {
                name: Some("Nikita".to_string()),
                email: None,
            },
            IdentityCandidate {
                name: Some("Other".to_string()),
                email: Some("x@y.com".to_string()),
            },
        ];

        let selected = build_preselected_identities(&current_identity, &[], &detected);

        assert!(
            !selected
                .iter()
                .any(|i| i.name.as_deref() == Some("Nikita") && i.email.is_none())
        );
    }

    #[test]
    fn import_is_deduped_when_commit_already_exists_from_hook_style_row() {
        let database = Database::open_in_memory().unwrap();
        let repo_path = "/tmp/repo";
        let repo_name = "repo";
        let branch = "main";
        let msg = "feat: subject\n\nbody";
        let committed_at = Utc.with_ymd_and_hms(2026, 2, 1, 12, 0, 0).unwrap();

        database
            .insert_commit(&Commit {
                id: None,
                hash: TEST_FULL_HASH.to_string(),
                message: msg.to_string(),
                repo_path: repo_path.to_string(),
                repo_name: repo_name.to_string(),
                branch: branch.to_string(),
                files_changed: 2,
                insertions: 1,
                deletions: 0,
                committed_at,
                author_email: Some("me@example.com".to_string()),
            })
            .unwrap();

        let scanned = ScannedCommit {
            hash: TEST_FULL_HASH.to_string(),
            message: msg.to_string(),
            author_name: "Me".to_string(),
            author_email: Some("me@example.com".to_string()),
            committed_at,
            committed_local_date: NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(),
        };

        let outcome =
            import_commits(&database, repo_path, repo_name, branch, &[scanned], 1, 1).unwrap();

        assert_eq!(database.commit_count().unwrap(), 1);
        assert_eq!(outcome.inserted, 0);
        assert_eq!(outcome.skipped_duplicates, 1);
    }

    #[test]
    fn cutoff_uses_commit_local_date_from_git_iso_offset() {
        let cutoff = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
        let selected = vec![IdentityCandidate {
            name: None,
            email: Some("me@example.com".to_string()),
        }];
        let dt = DateTime::parse_from_rfc3339("2026-01-02T04:00:00+09:00").unwrap();
        let committed_at = dt.with_timezone(&Utc);
        let commits = vec![ScannedCommit {
            hash: "x".to_string(),
            message: "m".to_string(),
            author_name: "Me".to_string(),
            author_email: Some("me@example.com".to_string()),
            committed_at,
            committed_local_date: dt.naive_local().date(),
        }];

        let imported = filter_importable_commits(&commits, cutoff, &selected);

        assert_eq!(imported.len(), 1);
    }

    #[test]
    fn identity_name_match_is_ascii_case_insensitive() {
        let cutoff = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let selected = vec![IdentityCandidate {
            name: Some("Me User".to_string()),
            email: None,
        }];
        let commits = vec![ScannedCommit {
            hash: "x".to_string(),
            message: "m".to_string(),
            author_name: "  me user  ".to_string(),
            author_email: None,
            committed_at: Utc.with_ymd_and_hms(2026, 1, 5, 12, 0, 0).unwrap(),
            committed_local_date: NaiveDate::from_ymd_opt(2026, 1, 5).unwrap(),
        }];

        let imported = filter_importable_commits(&commits, cutoff, &selected);

        assert_eq!(imported.len(), 1);
    }

    #[test]
    fn parse_identity_selection_prefers_preselected_when_line_empty() {
        let candidates = vec![IdentityCandidate {
            name: None,
            email: Some("a@b.com".to_string()),
        }];
        let preselected = vec![IdentityCandidate {
            name: Some("X".to_string()),
            email: Some("x@y.com".to_string()),
        }];

        let selected = super::parse_identity_selection("", &candidates, &preselected).unwrap();

        assert_eq!(selected, preselected);
    }

    #[test]
    fn parse_identity_selection_parses_indices_and_dedupes() {
        let candidates = vec![
            IdentityCandidate {
                name: None,
                email: Some("a@b.com".to_string()),
            },
            IdentityCandidate {
                name: None,
                email: Some("c@d.com".to_string()),
            },
        ];

        let selected = super::parse_identity_selection("2, 1, 2", &candidates, &[]).unwrap();

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].email.as_deref(), Some("c@d.com"));
        assert_eq!(selected[1].email.as_deref(), Some("a@b.com"));
    }

    #[test]
    fn parse_identity_selection_errors_on_invalid_index() {
        let candidates = vec![IdentityCandidate {
            name: None,
            email: Some("a@b.com".to_string()),
        }];

        assert!(super::parse_identity_selection("2", &candidates, &[]).is_err());
        assert!(super::parse_identity_selection("0", &candidates, &[]).is_err());
        assert!(super::parse_identity_selection("x", &candidates, &[]).is_err());
    }

    #[test]
    fn returns_empty_outcome_when_git_log_has_no_commits() {
        use std::collections::VecDeque;
        use std::io;

        let database = Database::open_in_memory().unwrap();
        let config_path = std::env::temp_dir().join("diddo-onboard-empty.toml");
        let config = config::AppConfig::default();

        let mut lines: VecDeque<String> = VecDeque::new();
        let mut read_line = move || Ok(lines.pop_front().map(|s| s + "\n").unwrap_or_default());
        let mut write_line = |_s: &str| -> io::Result<()> { Ok(()) };

        let mut run_git = |args: &[&str]| -> io::Result<String> {
            match args {
                ["rev-parse", "--show-toplevel"] => Ok("/tmp/repo\n".to_string()),
                ["rev-parse", "--abbrev-ref", "HEAD"] => Ok("main\n".to_string()),
                _ if args.first() == Some(&"log") => Ok(String::new()),
                _ => Err(io::Error::other(format!("unexpected git args: {args:?}"))),
            }
        };

        let outcome = run_with(
            &database,
            &config_path,
            config,
            &mut read_line,
            &mut write_line,
            &mut run_git,
        )
        .unwrap();

        assert_eq!(outcome.scanned, 0);
        assert_eq!(outcome.inserted, 0);
    }

    #[test]
    fn nothing_to_import_when_no_commits_on_or_after_cutoff() {
        use std::collections::VecDeque;
        use std::io;

        let database = Database::open_in_memory().unwrap();
        let config_path = std::env::temp_dir().join("diddo-onboard-before-cutoff.toml");
        let config = config::AppConfig::default();

        let mut lines: VecDeque<String> = VecDeque::from(["2026-01-01".to_string()]);
        let mut read_line = move || Ok(lines.pop_front().map(|s| s + "\n").unwrap_or_default());
        let mut write_line = |_s: &str| -> io::Result<()> { Ok(()) };

        let log = "h1\x1fMe\x1fme@example.com\x1f2025-12-31T12:00:00+00:00\x1fold\n\0";
        let mut run_git = move |args: &[&str]| -> io::Result<String> {
            match args {
                ["rev-parse", "--show-toplevel"] => Ok("/tmp/repo\n".to_string()),
                ["rev-parse", "--abbrev-ref", "HEAD"] => Ok("main\n".to_string()),
                _ if args.first() == Some(&"log") => Ok(log.to_string()),
                _ => Err(io::Error::other(format!("unexpected git args: {args:?}"))),
            }
        };

        let outcome = run_with(
            &database,
            &config_path,
            config,
            &mut read_line,
            &mut write_line,
            &mut run_git,
        )
        .unwrap();

        assert_eq!(outcome.scanned, 0);
        assert_eq!(outcome.inserted, 0);
    }

    #[test]
    fn onboarding_run_imports_only_user_confirmed_identities() {
        use std::collections::VecDeque;
        use std::io;

        let database = Database::open_in_memory().unwrap();
        let config_path = std::env::temp_dir().join("diddo-onboard-import.toml");
        let mut config = config::AppConfig::default();
        config.onboarding.save_selected_identities = false;

        let log = concat!(
            "h1\x1fMe\x1fme@example.com\x1f2026-01-01T12:00:00+00:00\x1fm1\n",
            "\0",
            "h2\x1fMe\x1fme@example.com\x1f2026-01-02T12:00:00+00:00\x1fm2\n",
            "\0",
            "h3\x1fOther\x1fother@example.com\x1f2026-01-03T12:00:00+00:00\x1fm3\n",
            "\0",
        )
        .to_string();

        let mut lines: VecDeque<String> =
            VecDeque::from(["2026-01-01".to_string(), "1".to_string()]);
        let mut read_line = move || Ok(lines.pop_front().map(|s| s + "\n").unwrap_or_default());
        let mut write_line = |_s: &str| -> io::Result<()> { Ok(()) };

        let mut run_git = move |args: &[&str]| -> io::Result<String> {
            match args {
                ["rev-parse", "--show-toplevel"] => Ok("/tmp/repo\n".to_string()),
                ["rev-parse", "--abbrev-ref", "HEAD"] => Ok("main\n".to_string()),
                ["config", "user.email"] => Ok("me@example.com\n".to_string()),
                ["config", "user.name"] => Ok("Me\n".to_string()),
                _ if args.first() == Some(&"log") => Ok(log.clone()),
                _ => Err(io::Error::other(format!("unexpected git args: {args:?}"))),
            }
        };

        let outcome = run_with(
            &database,
            &config_path,
            config,
            &mut read_line,
            &mut write_line,
            &mut run_git,
        )
        .unwrap();

        assert_eq!(outcome.scanned, 3);
        assert_eq!(outcome.matched, 2);
        assert_eq!(outcome.inserted, 2);
        assert_eq!(outcome.skipped_duplicates, 0);
    }
}
