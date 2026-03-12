#![allow(dead_code)]

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::io::{self, Write};

use crate::db::Commit;
use crate::summary_group::{ProfileGroup, RepoGroup};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryData {
    pub date_label: String,
    pub ai_summary: Option<String>,
    pub commits: Vec<Commit>,
    pub total_commits: usize,
    pub project_count: usize,
    pub first_commit_time: String,
    pub last_commit_time: String,
    pub most_active_project: String,
    pub most_active_count: usize,
}

/// Global stats for the summary footer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalStats {
    pub total_commits: usize,
    pub first_commit_time: String,
    pub last_commit_time: String,
    pub most_active_project: String,
    pub most_active_count: usize,
}

pub fn render_terminal(data: &SummaryData) {
    let mut stdout = io::stdout().lock();
    let _ = write_terminal(&mut stdout, data);
}

pub(crate) fn render_terminal_to_string(data: &SummaryData) -> String {
    let mut output = Vec::new();
    let _ = write_terminal(&mut output, data);
    String::from_utf8(output).unwrap_or_default()
}

pub fn render_markdown(data: &SummaryData) -> String {
    let mut output = format!("# {}\n\n", data.date_label);

    if let Some(summary) = data.ai_summary.as_deref() {
        output.push_str(summary.trim());
        output.push('\n');
    } else {
        output.push_str(&render_raw_markdown(&data.commits));
    }

    output.push_str(&format!(
        "\nFirst: {} | Last: {}\n",
        data.first_commit_time, data.last_commit_time,
    ));

    if !data.commits.is_empty() {
        output.push('\n');
        output.push_str(&render_markdown_table(&data.commits));
        output.push('\n');
    }

    output
}

pub fn render_json(data: &SummaryData) -> String {
    let projects = grouped_commits(&data.commits)
        .into_iter()
        .map(|project| {
            serde_json::json!({
                "repo_name": project.repo_name,
                "repo_path": project.repo_path,
                "commit_count": project.commits.len(),
                "commits": project
                    .commits
                    .into_iter()
                    .map(json_commit)
                    .collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();

    serde_json::to_string_pretty(&serde_json::json!({
        "date_label": data.date_label,
        "ai_summary": data.ai_summary,
        "projects": projects,
        "total_commits": data.total_commits,
        "project_count": data.project_count,
        "first_commit_time": data.first_commit_time,
        "last_commit_time": data.last_commit_time,
        "most_active_project": data.most_active_project,
        "most_active_count": data.most_active_count,
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

/// Renders summary by profile sections.
pub fn render_terminal_to_string_by_profile(
    sections: &[ProfileGroup],
    date_label: &str,
    global_stats: &GlobalStats,
) -> String {
    render_terminal_to_string_by_profile_with_table(sections, date_label, global_stats, true)
}

pub fn render_terminal_to_string_by_profile_with_table(
    sections: &[ProfileGroup],
    date_label: &str,
    global_stats: &GlobalStats,
    include_table: bool,
) -> String {
    let mut output = Vec::new();
    let _ = write_terminal_by_profile(
        &mut output,
        sections,
        date_label,
        global_stats,
        include_table,
    );
    String::from_utf8(output).unwrap_or_default()
}

/// Renders summary by profile sections.
pub fn render_markdown_by_profile(
    sections: &[ProfileGroup],
    date_label: &str,
    global_stats: &GlobalStats,
) -> String {
    render_markdown_by_profile_with_table(sections, date_label, global_stats, true)
}

pub fn render_markdown_by_profile_with_table(
    sections: &[ProfileGroup],
    date_label: &str,
    global_stats: &GlobalStats,
    include_table: bool,
) -> String {
    let mut output = format!("# {}\n\n", date_label);

    for section in sections {
        output.push_str(&format!("## Profile: {}\n\n", section.profile_label));
        if let Some(summary) = section.ai_summary.as_deref() {
            output.push_str(summary.trim());
            output.push('\n');
        } else {
            output.push_str(&repos_to_markdown(&section.repos));
        }
        output.push_str("\n\n");
    }

    if include_table {
        output.push_str(&format!(
            "First: {} | Last: {}\n\n",
            global_stats.first_commit_time, global_stats.last_commit_time,
        ));
        let all_commits = flatten_profile_commits(sections);
        if !all_commits.is_empty() {
            output.push_str(&render_markdown_table(&all_commits));
        }
    } else {
        output.push_str(&format!(
            "---\n{} commits | First: {} | Last: {} | Most active: {} ({})\n",
            global_stats.total_commits,
            global_stats.first_commit_time,
            global_stats.last_commit_time,
            global_stats.most_active_project,
            global_stats.most_active_count
        ));
    }

    output
}

/// Renders summary by profile sections.
pub fn render_json_by_profile(
    sections: &[ProfileGroup],
    date_label: &str,
    global_stats: &GlobalStats,
) -> String {
    let profiles: Vec<serde_json::Value> = sections
        .iter()
        .map(|section| {
            let repos: Vec<serde_json::Value> = section
                .repos
                .iter()
                .map(|repo| {
                    serde_json::json!({
                        "repo_name": repo.repo_name,
                        "repo_path": repo.repo_path,
                        "commit_count": repo.commits.len(),
                        "commits": repo.commits.iter().map(json_commit).collect::<Vec<_>>(),
                    })
                })
                .collect();
            serde_json::json!({
                "profile": section.profile_label,
                "ai_summary": section.ai_summary,
                "repos": repos,
            })
        })
        .collect();

    serde_json::to_string_pretty(&serde_json::json!({
        "date_label": date_label,
        "profiles": profiles,
        "total_commits": global_stats.total_commits,
        "first_commit_time": global_stats.first_commit_time,
        "last_commit_time": global_stats.last_commit_time,
        "most_active_project": global_stats.most_active_project,
        "most_active_count": global_stats.most_active_count,
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

/// Renders the table body (header, rows, total) as an ASCII table for terminal embedding.
pub(crate) fn render_terminal_table_body(commits: &[Commit]) -> String {
    let rows = repo_table_rows(commits);
    let total_commits = commits.len();
    let total_percentage = "100.0%";
    let repository_width = rows
        .iter()
        .map(|row| row.repository.len())
        .max()
        .unwrap_or(0)
        .max("repository".len())
        .max("Total".len());
    let commits_width = rows
        .iter()
        .map(|row| row.commit_count.to_string().len())
        .max()
        .unwrap_or(0)
        .max("commits".len())
        .max(total_commits.to_string().len());
    let percentage_width = rows
        .iter()
        .map(|row| row.percentage.len())
        .max()
        .unwrap_or(0)
        .max("percentage".len())
        .max(total_percentage.len());
    let separator = format!(
        "{:-<repository_width$}  {:-<commits_width$}  {:-<percentage_width$}",
        "", "", ""
    );

    let mut output = String::new();
    output.push_str(&format!(
        "{:<repository_width$}  {:>commits_width$}  {:>percentage_width$}\n",
        "repository", "commits", "percentage"
    ));
    output.push_str(&separator);
    output.push('\n');

    for row in rows {
        output.push_str(&format!(
            "{:<repository_width$}  {:>commits_width$}  {:>percentage_width$}\n",
            row.repository, row.commit_count, row.percentage
        ));
    }

    output.push_str(&separator);
    output.push('\n');
    output.push_str(&format!(
        "{:<repository_width$}  {:>commits_width$}  {:>percentage_width$}\n",
        "Total", total_commits, total_percentage
    ));
    output
}

/// Renders repository activity as a markdown table.
pub(crate) fn render_markdown_table(commits: &[Commit]) -> String {
    let rows = repo_table_rows(commits);
    let total_commits = commits.len();

    let mut output = String::new();
    output.push_str("| repository | commits | percentage |\n");
    output.push_str("| --- | ---: | ---: |\n");

    for row in rows {
        output.push_str(&format!(
            "| {} | {} | {} |\n",
            row.repository, row.commit_count, row.percentage
        ));
    }

    output.push_str(&format!(
        "| **Total** | **{}** | **100.0%** |\n",
        total_commits
    ));
    output
}

pub fn render_table(commits: &[Commit], date_label: &str) -> String {
    let mut output = String::new();
    output.push('\n');
    output.push_str(date_label);
    output.push_str("\n\n");
    output.push_str(&render_terminal_table_body(commits));
    output
}

fn write_terminal_by_profile<W: Write>(
    writer: &mut W,
    sections: &[ProfileGroup],
    date_label: &str,
    global_stats: &GlobalStats,
    include_table: bool,
) -> io::Result<()> {
    writeln!(writer)?;
    writeln!(writer, "{}", date_label)?;
    writeln!(writer)?;

    for section in sections {
        writeln!(writer, "Profile: {}", section.profile_label)?;
        if let Some(summary) = section.ai_summary.as_deref() {
            for line in summary.lines() {
                writeln!(writer, "{line}")?;
            }
        } else {
            write_repos_terminal(writer, &section.repos)?;
        }
        writeln!(writer)?;
    }

    if include_table {
        writeln!(writer, "First commit: {}", global_stats.first_commit_time)?;
        writeln!(writer, "Last commit: {}", global_stats.last_commit_time)?;
        writeln!(writer)?;
        let all_commits = flatten_profile_commits(sections);
        if !all_commits.is_empty() {
            write!(writer, "{}", render_terminal_table_body(&all_commits))?;
        }
    } else {
        writeln!(writer, "-----------------------")?;
        writeln!(writer, "{} commits", global_stats.total_commits)?;
        writeln!(writer, "First commit: {}", global_stats.first_commit_time)?;
        writeln!(writer, "Last commit: {}", global_stats.last_commit_time)?;
        writeln!(
            writer,
            "Most active: {} ({} {})",
            global_stats.most_active_project,
            global_stats.most_active_count,
            pluralize("commit", global_stats.most_active_count)
        )?;
    }
    writeln!(writer)
}

fn flatten_profile_commits(sections: &[ProfileGroup]) -> Vec<Commit> {
    sections
        .iter()
        .flat_map(|s| s.repos.iter().flat_map(|r| r.commits.iter().cloned()))
        .collect()
}

fn write_repos_terminal<W: Write>(writer: &mut W, repos: &[RepoGroup]) -> io::Result<()> {
    for repo in repos {
        writeln!(
            writer,
            "{} ({} {})",
            repo.repo_name,
            repo.commits.len(),
            pluralize("commit", repo.commits.len())
        )?;
        for commit in &repo.commits {
            writeln!(writer, "{}  {}", commit.hash, commit.message)?;
        }
        writeln!(writer)?;
    }
    Ok(())
}

fn repos_to_markdown(repos: &[RepoGroup]) -> String {
    let mut output = String::new();
    for repo in repos {
        output.push_str(&format!(
            "### {} ({} {})\n\n",
            repo.repo_name,
            repo.commits.len(),
            pluralize("commit", repo.commits.len())
        ));
        for commit in &repo.commits {
            output.push_str(&format!("- `{}` {}\n", commit.hash, commit.message));
        }
        output.push('\n');
    }
    output
}

fn write_terminal<W: Write>(writer: &mut W, data: &SummaryData) -> io::Result<()> {
    writeln!(writer)?;
    writeln!(writer, "{}", data.date_label)?;
    writeln!(writer)?;

    if let Some(summary) = data.ai_summary.as_deref() {
        for line in summary.lines() {
            writeln!(writer, "{line}")?;
        }
    } else {
        write_raw_terminal(writer, &data.commits)?;
    }

    writeln!(writer)?;
    writeln!(writer, "First commit: {}", data.first_commit_time)?;
    writeln!(writer, "Last commit: {}", data.last_commit_time)?;

    if !data.commits.is_empty() {
        writeln!(writer)?;
        write!(writer, "{}", render_terminal_table_body(&data.commits))?;
    }

    writeln!(writer)
}

fn write_raw_terminal<W: Write>(writer: &mut W, commits: &[Commit]) -> io::Result<()> {
    for project in grouped_commits(commits) {
        writeln!(
            writer,
            "{} ({} {})",
            project.repo_name,
            project.commits.len(),
            pluralize("commit", project.commits.len())
        )?;

        for commit in project.commits {
            writeln!(writer, "{}  {}", commit.hash, commit.message)?;
        }

        writeln!(writer)?;
    }

    Ok(())
}

fn render_raw_markdown(commits: &[Commit]) -> String {
    let mut output = String::new();

    for project in grouped_commits(commits) {
        output.push_str(&format!(
            "## {} ({} {})\n\n",
            project.repo_name,
            project.commits.len(),
            pluralize("commit", project.commits.len())
        ));

        for commit in project.commits {
            output.push_str(&format!("- `{}` {}\n", commit.hash, commit.message));
        }

        output.push('\n');
    }

    output
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectGroup<'a> {
    repo_name: &'a str,
    repo_path: &'a str,
    commits: Vec<&'a Commit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RepoTableRow {
    repository: String,
    commit_count: usize,
    percentage: String,
}

fn repo_table_rows(commits: &[Commit]) -> Vec<RepoTableRow> {
    let project_groups = grouped_commits(commits);
    let total_commits = commits.len();
    let mut repo_name_counts = BTreeMap::new();

    for project in &project_groups {
        *repo_name_counts.entry(project.repo_name).or_insert(0usize) += 1;
    }

    project_groups
        .into_iter()
        .map(|project| {
            let repository = if repo_name_counts
                .get(project.repo_name)
                .copied()
                .unwrap_or(0)
                > 1
            {
                format!("{} ({})", project.repo_name, project.repo_path)
            } else {
                project.repo_name.to_string()
            };
            let commit_count = project.commits.len();

            RepoTableRow {
                repository,
                commit_count,
                percentage: format!("{:.1}%", commit_count as f64 * 100.0 / total_commits as f64),
            }
        })
        .collect()
}

fn grouped_commits(commits: &[Commit]) -> Vec<ProjectGroup<'_>> {
    let mut groups: Vec<ProjectGroup<'_>> = Vec::new();

    for commit in commits {
        if let Some(project) = groups.iter_mut().find(|project| {
            project.repo_name == commit.repo_name.as_str()
                && project.repo_path == commit.repo_path.as_str()
        }) {
            project.commits.push(commit);
        } else {
            groups.push(ProjectGroup {
                repo_name: commit.repo_name.as_str(),
                repo_path: commit.repo_path.as_str(),
                commits: vec![commit],
            });
        }
    }

    for project in &mut groups {
        project
            .commits
            .sort_by(|left, right| commit_sort_key(left).cmp(&commit_sort_key(right)));
    }

    groups.sort_by(|left, right| {
        (
            Reverse(left.commits.len()),
            left.repo_name,
            left.repo_path,
            first_commit_sort_key(&left.commits),
        )
            .cmp(&(
                Reverse(right.commits.len()),
                right.repo_name,
                right.repo_path,
                first_commit_sort_key(&right.commits),
            ))
    });

    groups
}

fn first_commit_sort_key<'a>(
    commits: &'a [&'a Commit],
) -> (&'a chrono::DateTime<chrono::Utc>, &'a str, &'a str) {
    let commit = commits
        .first()
        .expect("project groups should always contain at least one commit");

    (
        &commit.committed_at,
        commit.hash.as_str(),
        commit.message.as_str(),
    )
}

fn commit_sort_key(commit: &Commit) -> (&chrono::DateTime<chrono::Utc>, &str, &str) {
    (
        &commit.committed_at,
        commit.hash.as_str(),
        commit.message.as_str(),
    )
}

fn json_commit(commit: &Commit) -> serde_json::Value {
    serde_json::json!({
        "id": commit.id,
        "hash": commit.hash,
        "message": commit.message,
        "repo_path": commit.repo_path,
        "repo_name": commit.repo_name,
        "branch": commit.branch,
        "files_changed": commit.files_changed,
        "insertions": commit.insertions,
        "deletions": commit.deletions,
        "committed_at": commit.committed_at.to_rfc3339(),
    })
}

fn pluralize(word: &str, count: usize) -> &str {
    if count == 1 {
        word
    } else {
        match word {
            "commit" => "commits",
            _ => word,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::Value;

    use super::{
        render_json, render_markdown, render_markdown_by_profile, render_terminal_to_string,
        render_terminal_to_string_by_profile, write_terminal, GlobalStats, SummaryData,
    };
    use crate::db::Commit;
    use crate::summary_group::{ProfileGroup, RepoGroup};

    #[test]
    fn terminal_renders_ai_summary_and_stats() {
        let data = sample_summary(Some("Wrapped up hook integration.\nKept config stable."));
        let mut output = Vec::new();

        write_terminal(&mut output, &data).unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.contains("\n2026-03-10 (today)\n"));
        assert!(rendered.contains("Wrapped up hook integration."));
        assert!(rendered.contains("Kept config stable."));
        assert!(!rendered.contains("3 commits across 2 projects"));
        assert!(rendered.contains("First commit: 09:15"));
        assert!(rendered.contains("Last commit: 15:20"));
        assert!(!rendered.contains("Most active: diddo (2 commits)"));
    }

    #[test]
    fn terminal_groups_raw_commits_by_repo_without_ai_summary() {
        let data = sample_summary(None);
        let mut output = Vec::new();

        write_terminal(&mut output, &data).unwrap();

        let rendered = String::from_utf8(output).unwrap();
        let diddo_index = rendered.find("diddo (2 commits)").unwrap();
        let api_service_index = rendered.find("api-service (1 commit)").unwrap();

        assert!(diddo_index < api_service_index);
        assert!(rendered.contains("abc1234  feat: add renderers"));
        assert!(rendered.contains("def5678  test: cover json output"));
        assert!(rendered.contains("987zyx1  fix: handle empty summary"));
    }

    #[test]
    fn terminal_breaks_group_order_ties_by_repo_name() {
        let data = tied_summary(None);
        let mut output = Vec::new();

        write_terminal(&mut output, &data).unwrap();

        let rendered = String::from_utf8(output).unwrap();
        let alpha_index = rendered.find("alpha-app (2 commits)").unwrap();
        let zebra_index = rendered.find("zebra-app (2 commits)").unwrap();

        assert!(alpha_index < zebra_index);
    }

    #[test]
    fn markdown_groups_raw_commits_and_appends_summary_stats() {
        let data = sample_summary(None);

        let rendered = render_markdown(&data);

        assert!(rendered.starts_with("# 2026-03-10 (today)\n"));
        let diddo_index = rendered.find("## diddo (2 commits)").unwrap();
        let api_service_index = rendered.find("## api-service (1 commit)").unwrap();

        assert!(diddo_index < api_service_index);
        assert!(rendered.contains("- `abc1234` feat: add renderers"));
        assert!(rendered.contains("First: 09:15 | Last: 15:20"));
        assert!(!rendered.contains("3 commits across 2 projects"));
    }

    #[test]
    fn json_renders_pretty_serialized_summary_data() {
        let data = sample_summary(Some("Wrapped up hook integration."));

        let rendered = render_json(&data);
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert!(rendered.contains("\n  \"date_label\""));
        assert_eq!(value["date_label"], "2026-03-10 (today)");
        assert_eq!(value["ai_summary"], "Wrapped up hook integration.");
        assert_eq!(value["total_commits"], 3);
        assert_eq!(value["project_count"], 2);
        assert_eq!(value["first_commit_time"], "09:15");
        assert_eq!(value["last_commit_time"], "15:20");
        assert_eq!(value["most_active_project"], "diddo");
        assert_eq!(value["most_active_count"], 2);
        assert_eq!(value["projects"].as_array().unwrap().len(), 2);
        assert_eq!(value["projects"][0]["repo_name"], "diddo");
        assert_eq!(value["projects"][0]["commit_count"], 2);
        assert_eq!(value["projects"][0]["commits"].as_array().unwrap().len(), 2);
        assert_eq!(
            value["projects"][0]["commits"][0]["message"],
            "feat: add renderers"
        );
        assert_eq!(value["projects"][1]["repo_name"], "api-service");
        assert!(value.get("commits").is_none());
    }

    #[test]
    fn json_sorts_projects_by_commit_count_then_repo_name() {
        let data = tied_summary(None);

        let rendered = render_json(&data);
        let value: Value = serde_json::from_str(&rendered).unwrap();

        let projects = value["projects"].as_array().unwrap();

        assert_eq!(projects[0]["repo_name"], "alpha-app");
        assert_eq!(projects[1]["repo_name"], "zebra-app");
    }

    fn default_global_stats() -> GlobalStats {
        GlobalStats {
            total_commits: 1,
            first_commit_time: "09:15".to_string(),
            last_commit_time: "15:20".to_string(),
            most_active_project: "my-repo".to_string(),
            most_active_count: 1,
        }
    }

    #[test]
    fn by_profile_one_profile_one_repo_with_ai_summary() {
        let commit = sample_commit("abc123", "feat: add x", "my-repo", 9, 15);
        let groups: Vec<ProfileGroup> = vec![ProfileGroup {
            profile_label: "dev@example.com".to_string(),
            repos: vec![RepoGroup {
                repo_name: "my-repo".to_string(),
                repo_path: "/path/my-repo".to_string(),
                commits: vec![commit],
            }],
            ai_summary: Some("Shipped the new feature.".to_string()),
        }];
        let stats = default_global_stats();

        let out = render_terminal_to_string_by_profile(&groups, "2026-03-10 (today)", &stats);
        assert!(out.contains("Profile: dev@example.com"));
        assert!(out.contains("Shipped the new feature."));

        let md = render_markdown_by_profile(&groups, "2026-03-10 (today)", &stats);
        assert!(md.contains("## Profile: dev@example.com"));
        assert!(md.contains("Shipped the new feature."));
    }

    #[test]
    fn by_profile_two_profiles_two_sections() {
        let c1 = sample_commit("a1", "msg a", "repo-a", 9, 0);
        let c2 = sample_commit("b1", "msg b", "repo-b", 10, 0);
        let groups: Vec<ProfileGroup> = vec![
            ProfileGroup {
                profile_label: "alice@x.com".to_string(),
                repos: vec![RepoGroup {
                    repo_name: "repo-a".to_string(),
                    repo_path: "/path/a".to_string(),
                    commits: vec![c1],
                }],
                ai_summary: Some("Alice summary.".to_string()),
            },
            ProfileGroup {
                profile_label: "bob@y.com".to_string(),
                repos: vec![RepoGroup {
                    repo_name: "repo-b".to_string(),
                    repo_path: "/path/b".to_string(),
                    commits: vec![c2],
                }],
                ai_summary: Some("Bob summary.".to_string()),
            },
        ];
        let stats = GlobalStats {
            total_commits: 2,
            first_commit_time: "09:00".to_string(),
            last_commit_time: "10:00".to_string(),
            most_active_project: "repo-a".to_string(),
            most_active_count: 1,
        };

        let out = render_terminal_to_string_by_profile(&groups, "2026-03-10 (today)", &stats);
        let pos_alice = out.find("Profile: alice@x.com").unwrap();
        let pos_bob = out.find("Profile: bob@y.com").unwrap();
        assert!(pos_alice < pos_bob);
        assert!(out.contains("Alice summary."));
        assert!(out.contains("Bob summary."));
    }

    #[test]
    fn by_profile_one_profile_no_ai_summary_raw_repo_list() {
        let c1 = sample_commit("h1", "first commit", "proj", 9, 15);
        let c2 = sample_commit("h2", "second commit", "proj", 10, 30);
        let groups: Vec<ProfileGroup> = vec![ProfileGroup {
            profile_label: "unknown".to_string(),
            repos: vec![RepoGroup {
                repo_name: "proj".to_string(),
                repo_path: "/tmp/proj".to_string(),
                commits: vec![c1, c2],
            }],
            ai_summary: None,
        }];
        let stats = default_global_stats();

        let out = render_terminal_to_string_by_profile(&groups, "2026-03-10 (today)", &stats);
        assert!(out.contains("Profile: unknown"));
        assert!(out.contains("proj (2 commits)"));
        assert!(out.contains("h1  first commit"));
        assert!(out.contains("h2  second commit"));

        let md = render_markdown_by_profile(&groups, "2026-03-10 (today)", &stats);
        assert!(md.contains("## Profile: unknown"));
        assert!(md.contains("### proj (2 commits)"));
        assert!(md.contains("- `h1` first commit"));
        assert!(md.contains("- `h2` second commit"));
    }

    #[test]
    fn markdown_table_renders_repo_counts_percentages_and_total() {
        let commits = sample_summary(None).commits;
        let rendered = super::render_markdown_table(&commits);

        assert!(rendered.contains("| repository | commits | percentage |"));
        assert!(rendered.contains("| --- | ---: | ---: |"));
        assert!(rendered.contains("| diddo | 2 | 66.7% |"));
        assert!(rendered.contains("| api-service | 1 | 33.3% |"));
        assert!(rendered.contains("| **Total** | **3** | **100.0%** |"));
    }

    #[test]
    fn terminal_table_body_can_be_embedded_after_summary_text() {
        let commits = sample_summary(None).commits;
        let rendered = super::render_terminal_table_body(&commits);

        let embedded = format!("AI summary here.\n\n{rendered}");

        assert!(embedded.starts_with("AI summary here.\n\nrepository"));
        assert!(rendered.contains("repository"));
        assert!(rendered.contains("commits"));
        assert!(rendered.contains("percentage"));
        assert!(rendered.contains("diddo"));
        assert!(rendered.contains("api-service"));
        assert!(rendered.contains("Total"));
        assert!(rendered.contains("100.0%"));
        assert!(!rendered.contains("2026"));
        assert!(!rendered.contains("AI summary here."));
    }

    #[test]
    fn terminal_summary_with_ai_appends_table_before_footer() {
        let data = sample_summary(Some("AI summary here."));
        let rendered = render_terminal_to_string(&data);

        let ai_pos = rendered.find("AI summary here.").unwrap();
        let first_commit_pos = rendered.find("First commit:").unwrap();
        let table_pos = rendered.find("repository").unwrap();
        assert!(ai_pos < first_commit_pos);
        assert!(first_commit_pos < table_pos);
        assert!(rendered.contains("Total"));
        assert!(rendered.contains("100.0%"));
    }

    #[test]
    fn terminal_summary_without_ai_appends_table_after_first_last() {
        let data = sample_summary(None);
        let rendered = render_terminal_to_string(&data);

        let raw_pos = rendered.find("diddo (2 commits)").unwrap();
        let first_commit_pos = rendered.find("First commit:").unwrap();
        let table_pos = rendered.find("repository").unwrap();
        assert!(raw_pos < first_commit_pos);
        assert!(first_commit_pos < table_pos);
    }

    #[test]
    fn markdown_summary_appends_table_after_first_last() {
        let data = sample_summary(Some("AI markdown summary."));
        let rendered = render_markdown(&data);

        let ai_pos = rendered.find("AI markdown summary.").unwrap();
        let stats_pos = rendered.find("First:").unwrap();
        let table_pos = rendered.find("| repository |").unwrap();
        assert!(ai_pos < stats_pos);
        assert!(stats_pos < table_pos);
        assert!(rendered.contains("| **Total** |"));
    }

    #[test]
    fn by_profile_terminal_appends_table_after_first_last() {
        let commit = sample_commit("abc123", "feat: add x", "my-repo", 9, 15);
        let groups: Vec<ProfileGroup> = vec![ProfileGroup {
            profile_label: "dev@example.com".to_string(),
            repos: vec![RepoGroup {
                repo_name: "my-repo".to_string(),
                repo_path: "/path/my-repo".to_string(),
                commits: vec![commit],
            }],
            ai_summary: Some("Shipped the new feature.".to_string()),
        }];
        let stats = default_global_stats();

        let out = render_terminal_to_string_by_profile(&groups, "2026-03-10 (today)", &stats);
        let summary_pos = out.find("Shipped the new feature.").unwrap();
        let first_commit_pos = out.find("First commit:").unwrap();
        let table_pos = out.find("repository").unwrap();
        assert!(summary_pos < first_commit_pos);
        assert!(first_commit_pos < table_pos);
        assert!(out.contains("Total"));
        assert!(out.contains("100.0%"));
    }

    #[test]
    fn by_profile_markdown_appends_table_after_first_last() {
        let commit = sample_commit("abc123", "feat: add x", "my-repo", 9, 15);
        let groups: Vec<ProfileGroup> = vec![ProfileGroup {
            profile_label: "dev@example.com".to_string(),
            repos: vec![RepoGroup {
                repo_name: "my-repo".to_string(),
                repo_path: "/path/my-repo".to_string(),
                commits: vec![commit],
            }],
            ai_summary: Some("Shipped the new feature.".to_string()),
        }];
        let stats = default_global_stats();

        let md = render_markdown_by_profile(&groups, "2026-03-10 (today)", &stats);
        let summary_pos = md.find("Shipped the new feature.").unwrap();
        let first_pos = md.find("First:").unwrap();
        let table_pos = md.find("| repository |").unwrap();
        assert!(summary_pos < first_pos);
        assert!(first_pos < table_pos);
        assert!(md.contains("| **Total** |"));
    }

    #[test]
    fn table_renders_sorted_repo_counts_percentages_and_total_row() {
        let rendered = super::render_table(&sample_summary(None).commits, "2026-03-10 (today)");

        assert!(rendered.contains("2026-03-10 (today)"));
        assert!(rendered.contains("repository"));
        assert!(rendered.contains("commits"));
        assert!(rendered.contains("percentage"));
        let diddo_index = rendered.find("diddo").unwrap();
        let api_service_index = rendered.find("api-service").unwrap();
        let total_index = rendered.find("Total").unwrap();
        assert!(diddo_index < api_service_index);
        assert!(api_service_index < total_index);
        assert!(rendered.contains("66.7%"));
        assert!(rendered.contains("33.3%"));
        assert!(rendered.contains("100.0%"));
    }

    #[test]
    fn table_breaks_ties_by_repo_name() {
        let rendered = super::render_table(&tied_summary(None).commits, "2026-03-10 (today)");

        let alpha_index = rendered.find("alpha-app").unwrap();
        let zebra_index = rendered.find("zebra-app").unwrap();
        assert!(alpha_index < zebra_index);
    }

    fn sample_summary(ai_summary: Option<&str>) -> SummaryData {
        SummaryData {
            date_label: "2026-03-10 (today)".to_string(),
            ai_summary: ai_summary.map(str::to_string),
            commits: vec![
                sample_commit(
                    "987zyx1",
                    "fix: handle empty summary",
                    "api-service",
                    15,
                    20,
                ),
                sample_commit("abc1234", "feat: add renderers", "diddo", 9, 15),
                sample_commit("def5678", "test: cover json output", "diddo", 10, 45),
            ],
            total_commits: 3,
            project_count: 2,
            first_commit_time: "09:15".to_string(),
            last_commit_time: "15:20".to_string(),
            most_active_project: "diddo".to_string(),
            most_active_count: 2,
        }
    }

    fn tied_summary(ai_summary: Option<&str>) -> SummaryData {
        SummaryData {
            date_label: "2026-03-10 (today)".to_string(),
            ai_summary: ai_summary.map(str::to_string),
            commits: vec![
                sample_commit("zeb0001", "feat: add zebra dashboard", "zebra-app", 9, 15),
                sample_commit("alp0001", "feat: add alpha dashboard", "alpha-app", 10, 0),
                sample_commit("zeb0002", "fix: polish zebra ui", "zebra-app", 11, 30),
                sample_commit("alp0002", "test: cover alpha ui", "alpha-app", 13, 45),
            ],
            total_commits: 4,
            project_count: 2,
            first_commit_time: "09:15".to_string(),
            last_commit_time: "13:45".to_string(),
            most_active_project: "alpha-app".to_string(),
            most_active_count: 2,
        }
    }

    fn sample_commit(hash: &str, message: &str, repo_name: &str, hour: u32, minute: u32) -> Commit {
        Commit {
            id: None,
            hash: hash.to_string(),
            message: message.to_string(),
            repo_path: format!("/tmp/{repo_name}"),
            repo_name: repo_name.to_string(),
            branch: "main".to_string(),
            files_changed: 3,
            insertions: 12,
            deletions: 4,
            committed_at: Utc.with_ymd_and_hms(2026, 3, 10, hour, minute, 0).unwrap(),
            author_email: None,
        }
    }
}
