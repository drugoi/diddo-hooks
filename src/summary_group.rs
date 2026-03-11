//! Groups commits by profile (author email) then by repo for summary output.

use crate::db::Commit;
use std::collections::BTreeMap;

/// Commits belonging to a single repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoGroup {
    pub repo_name: String,
    pub repo_path: String,
    pub commits: Vec<Commit>,
}

/// Profile (author email) with its repo groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileGroup {
    pub profile_label: String,
    pub repos: Vec<RepoGroup>,
    /// AI-generated summary for this profile (set after summarization).
    pub ai_summary: Option<String>,
}

/// Normalizes author_email to a profile key: trim; if empty use "unknown".
fn profile_key(commit: &Commit) -> String {
    let key = commit
        .author_email
        .as_deref()
        .unwrap_or("")
        .trim();
    if key.is_empty() {
        "unknown".to_string()
    } else {
        key.to_string()
    }
}

/// Groups commits by (profile_key, repo_path), then builds ProfileGroups with repos
/// sorted by repo_name and commits by committed_at then hash.
pub fn group_commits_by_profile_then_repo(commits: &[Commit]) -> Vec<ProfileGroup> {
    // (profile_key, repo_path) -> (repo_name, commits)
    let mut by_profile_repo: BTreeMap<String, BTreeMap<String, (String, Vec<Commit>)>> =
        BTreeMap::new();

    for commit in commits {
        let profile = profile_key(commit);
        let repo_path = commit.repo_path.clone();
        let repo_name = commit.repo_name.clone();

        by_profile_repo
            .entry(profile)
            .or_default()
            .entry(repo_path.clone())
            .or_insert_with(|| (repo_name.clone(), Vec::new()))
            .1
            .push(commit.clone());
    }

    let mut out = Vec::with_capacity(by_profile_repo.len());
    for (profile_label, repos_map) in by_profile_repo {
        let mut repos: Vec<RepoGroup> = repos_map
            .into_iter()
            .map(|(repo_path, (repo_name, mut commits))| {
                commits.sort_by(|a, b| {
                    a.committed_at
                        .cmp(&b.committed_at)
                        .then_with(|| a.hash.cmp(&b.hash))
                });
                RepoGroup {
                    repo_name,
                    repo_path,
                    commits,
                }
            })
            .collect();
        repos.sort_by(|a, b| a.repo_name.cmp(&b.repo_name));
        out.push(ProfileGroup {
            profile_label,
            repos,
            ai_summary: None,
        });
    }
    out.sort_by(|a, b| a.profile_label.cmp(&b.profile_label));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    fn make_commit(
        hash: &str,
        author_email: Option<&str>,
        repo_name: &str,
        repo_path: &str,
        committed_at: DateTime<Utc>,
    ) -> Commit {
        Commit {
            id: None,
            hash: hash.to_string(),
            message: "msg".to_string(),
            repo_path: repo_path.to_string(),
            repo_name: repo_name.to_string(),
            branch: "main".to_string(),
            files_changed: 0,
            insertions: 0,
            deletions: 0,
            committed_at,
            author_email: author_email.map(String::from),
        }
    }

    #[test]
    fn group_commits_by_profile_then_repo_two_profiles_two_repos() {
        let t1 = DateTime::parse_from_rfc3339("2026-03-11T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let t2 = DateTime::parse_from_rfc3339("2026-03-11T11:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let commits = vec![
            make_commit("a1", Some("a@x.com"), "repo1", "/path/repo1", t1),
            make_commit("a2", Some("a@x.com"), "repo1", "/path/repo1", t2),
            make_commit("a3", Some("a@x.com"), "repo2", "/path/repo2", t1),
            make_commit("b1", Some("b@y.com"), "repo1", "/path/repo1", t1),
            make_commit("b2", Some("b@y.com"), "repo2", "/path/repo2", t2),
        ];

        let groups = group_commits_by_profile_then_repo(&commits);

        assert_eq!(groups.len(), 2, "two profiles");

        let first = &groups[0];
        let second = &groups[1];

        assert_eq!(first.profile_label, "a@x.com");
        assert_eq!(first.repos.len(), 2);
        let a_repo1 = first.repos.iter().find(|r| r.repo_name == "repo1").unwrap();
        let a_repo2 = first.repos.iter().find(|r| r.repo_name == "repo2").unwrap();
        assert_eq!(a_repo1.commits.len(), 2);
        assert_eq!(a_repo2.commits.len(), 1);

        assert_eq!(second.profile_label, "b@y.com");
        assert_eq!(second.repos.len(), 2);
        let b_repo1 = second.repos.iter().find(|r| r.repo_name == "repo1").unwrap();
        let b_repo2 = second.repos.iter().find(|r| r.repo_name == "repo2").unwrap();
        assert_eq!(b_repo1.commits.len(), 1);
        assert_eq!(b_repo2.commits.len(), 1);
    }
}
