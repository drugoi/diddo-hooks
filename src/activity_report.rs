use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Datelike, Local, NaiveDate, Weekday};

use crate::db::Commit;

#[derive(Debug, Clone, PartialEq)]
pub struct ActivityReport {
    pub period_months: u32,
    pub period_label: String,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub heatmap: BTreeMap<NaiveDate, usize>,
    pub top_weekdays: Vec<(String, usize)>,
    pub avg_files_per_commit: f64,
    pub avg_commits_per_day: f64,
    pub top_repos: Vec<(String, usize)>,
    pub total_commits: usize,
    pub total_days: u64,
}

pub const PERIOD_OPTIONS: &[(u32, &str)] = &[
    (12, "Last year"),
    (6, "6 months"),
    (3, "3 months"),
    (1, "1 month"),
];

pub fn compute_period_range(months: u32, today: NaiveDate) -> (NaiveDate, NaiveDate) {
    let from = shift_months_back(today, months);
    (from, today)
}

fn shift_months_back(date: NaiveDate, months: u32) -> NaiveDate {
    let mut year = date.year();
    let mut month = date.month() as i32 - months as i32;

    while month <= 0 {
        month += 12;
        year -= 1;
    }

    let day = date.day().min(days_in_month(year, month as u32));
    NaiveDate::from_ymd_opt(year, month as u32, day).unwrap_or(date)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    NaiveDate::from_ymd_opt(
        if month == 12 { year + 1 } else { year },
        if month == 12 { 1 } else { month + 1 },
        1,
    )
    .unwrap()
    .pred_opt()
    .unwrap()
    .day()
}

pub fn build_report(
    commits: &[Commit],
    from: NaiveDate,
    to: NaiveDate,
    period_months: u32,
) -> ActivityReport {
    let total_days = (to - from).num_days().max(1) as u64;
    let total_commits = commits.len();

    // Build heatmap: count commits per date (in local timezone)
    let mut heatmap = BTreeMap::new();
    for commit in commits {
        let local_date = commit.committed_at.with_timezone(&Local).date_naive();
        *heatmap.entry(local_date).or_insert(0usize) += 1;
    }

    // Weekday counts
    let mut weekday_counts: [usize; 7] = [0; 7];
    for commit in commits {
        let local_date = commit.committed_at.with_timezone(&Local).date_naive();
        let idx = weekday_index(local_date.weekday());
        weekday_counts[idx] += 1;
    }

    let weekday_names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let mut weekday_pairs: Vec<(String, usize)> = weekday_names
        .iter()
        .enumerate()
        .map(|(i, name)| (name.to_string(), weekday_counts[i]))
        .collect();
    weekday_pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let top_weekdays: Vec<(String, usize)> = weekday_pairs.into_iter().take(3).collect();

    // Avg files per commit
    let avg_files_per_commit = if total_commits > 0 {
        commits.iter().map(|c| c.files_changed as f64).sum::<f64>() / total_commits as f64
    } else {
        0.0
    };

    // Avg commits per day
    let avg_commits_per_day = total_commits as f64 / total_days as f64;

    // Top repos
    let mut repo_counts: BTreeMap<String, usize> = BTreeMap::new();
    for commit in commits {
        *repo_counts.entry(commit.repo_name.clone()).or_insert(0) += 1;
    }
    let mut repo_pairs: Vec<(String, usize)> = repo_counts.into_iter().collect();
    repo_pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let top_repos: Vec<(String, usize)> = repo_pairs.into_iter().take(3).collect();

    let period_label = PERIOD_OPTIONS
        .iter()
        .find(|(m, _)| *m == period_months)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| format!("{period_months} months"));

    ActivityReport {
        period_months,
        period_label,
        from,
        to,
        heatmap,
        top_weekdays,
        avg_files_per_commit,
        avg_commits_per_day,
        top_repos,
        total_commits,
        total_days,
    }
}

fn weekday_index(wd: Weekday) -> usize {
    match wd {
        Weekday::Mon => 0,
        Weekday::Tue => 1,
        Weekday::Wed => 2,
        Weekday::Thu => 3,
        Weekday::Fri => 4,
        Weekday::Sat => 5,
        Weekday::Sun => 6,
    }
}

fn heatmap_char(count: usize) -> char {
    match count {
        0 => '·',
        1..=2 => '░',
        3..=5 => '▒',
        6..=9 => '▓',
        _ => '█',
    }
}

fn heatmap_ansi_color(count: usize) -> &'static str {
    match count {
        0 => "\x1b[90m",     // dim gray
        1..=2 => "\x1b[32m", // green
        3..=5 => "\x1b[33m", // yellow
        6..=9 => "\x1b[35m", // magenta
        _ => "\x1b[31m",     // red
    }
}

const ANSI_RESET: &str = "\x1b[0m";

pub fn render_terminal(report: &ActivityReport) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "Activity Report — {} ({} to {})\n",
        report.period_label, report.from, report.to
    ));
    out.push_str(&format!(
        "{} commits over {} days\n\n",
        report.total_commits, report.total_days
    ));

    // Heatmap (colored)
    out.push_str(&render_heatmap_terminal(report));
    out.push('\n');

    // Stats
    out.push_str("Top active days of week\n");
    out.push_str("───────────────────────\n");
    for (day, count) in &report.top_weekdays {
        out.push_str(&format!("  {day:<3}  {count} commits\n"));
    }
    out.push('\n');

    out.push_str("Statistics\n");
    out.push_str("──────────\n");
    out.push_str(&format!(
        "  Avg files per commit   {:.1}\n",
        report.avg_files_per_commit
    ));
    out.push_str(&format!(
        "  Avg commits per day    {:.1}\n",
        report.avg_commits_per_day
    ));
    out.push('\n');

    out.push_str("Top repositories\n");
    out.push_str("────────────────\n");
    for (repo, count) in &report.top_repos {
        out.push_str(&format!("  {repo:<30}  {count} commits\n"));
    }

    out
}

struct HeatmapGrid {
    start: NaiveDate,
    total_weeks: usize,
    month_label_row: String,
}

fn compute_heatmap_grid(report: &ActivityReport) -> HeatmapGrid {
    let start = prev_monday(report.from);
    let end = next_sunday(report.to);

    let total_days = (end - start).num_days() + 1;
    let total_weeks = (total_days as usize).div_ceil(7);

    // Month labels
    let mut month_labels: Vec<(usize, String)> = Vec::new();
    let mut prev_month = 0u32;
    let mut day = start;
    for week_idx in 0..total_weeks {
        let week_start = day;
        if week_start.month() != prev_month {
            month_labels.push((week_idx, format_month_short(week_start.month())));
            prev_month = week_start.month();
        }
        day += chrono::Duration::days(7);
    }

    let mut label_row = vec![' '; total_weeks * 2];
    for (col, label) in &month_labels {
        let pos = col * 2;
        for (i, ch) in label.chars().enumerate() {
            if pos + i < label_row.len() {
                label_row[pos + i] = ch;
            }
        }
    }
    let month_label_row: String = label_row.into_iter().collect();

    HeatmapGrid {
        start,
        total_weeks,
        month_label_row,
    }
}

const DAY_LABELS: [&str; 7] = ["Mon", "   ", "Wed", "   ", "Fri", "   ", "Sun"];

fn render_heatmap_terminal(report: &ActivityReport) -> String {
    let grid = compute_heatmap_grid(report);
    let mut out = String::new();

    // Month labels
    out.push_str("     ");
    out.push_str(grid.month_label_row.trim_end());
    out.push('\n');

    // Rows
    for (row, day_label) in DAY_LABELS.iter().enumerate() {
        out.push_str(&format!("{day_label} "));
        for week in 0..grid.total_weeks {
            let cell_date = grid.start + chrono::Duration::days((week * 7 + row) as i64);
            if cell_date >= report.from && cell_date <= report.to {
                let count = report.heatmap.get(&cell_date).copied().unwrap_or(0);
                let color = heatmap_ansi_color(count);
                let ch = if count == 0 { '·' } else { '█' };
                out.push_str(&format!("{color}{ch}{ANSI_RESET} "));
            } else {
                out.push_str("  ");
            }
        }
        out.push('\n');
    }

    // Colored legend
    out.push_str("\n     \x1b[90m·\x1b[0m none  \x1b[32m█\x1b[0m 1-2  \x1b[33m█\x1b[0m 3-5  \x1b[35m█\x1b[0m 6-9  \x1b[31m█\x1b[0m 10+\n");

    out
}

fn render_heatmap_markdown(report: &ActivityReport) -> String {
    let grid = compute_heatmap_grid(report);
    let mut out = String::new();

    // Month labels
    out.push_str("     ");
    out.push_str(grid.month_label_row.trim_end());
    out.push('\n');

    // Rows
    for (row, day_label) in DAY_LABELS.iter().enumerate() {
        out.push_str(&format!("{day_label} "));
        for week in 0..grid.total_weeks {
            let cell_date = grid.start + chrono::Duration::days((week * 7 + row) as i64);
            if cell_date >= report.from && cell_date <= report.to {
                let count = report.heatmap.get(&cell_date).copied().unwrap_or(0);
                out.push(heatmap_char(count));
            } else {
                out.push(' ');
            }
            out.push(' ');
        }
        out.push('\n');
    }

    // Plain legend
    out.push_str("\n     · none  ░ 1-2  ▒ 3-5  ▓ 6-9  █ 10+\n");

    out
}

fn prev_monday(date: NaiveDate) -> NaiveDate {
    let wd = date.weekday().num_days_from_monday(); // Mon=0, Sun=6
    date - chrono::Duration::days(wd as i64)
}

fn next_sunday(date: NaiveDate) -> NaiveDate {
    let wd = date.weekday().num_days_from_monday(); // Mon=0, Sun=6
    let days_to_sunday = if wd == 6 { 0 } else { 6 - wd };
    date + chrono::Duration::days(days_to_sunday as i64)
}

fn escape_markdown_table_cell(text: &str) -> String {
    text.replace('|', "\\|")
}

fn unique_export_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("diddo_activity_export.md");
    let (stem, ext) = file_name.rsplit_once('.').unwrap_or((file_name, "md"));
    for n in 2..=9999 {
        let candidate = parent.join(format!("{stem}_{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem}_{}.{ext}", std::process::id()))
}

fn format_month_short(month: u32) -> String {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "???",
    }
    .to_string()
}

pub fn render_markdown(report: &ActivityReport) -> String {
    let mut out = String::new();

    out.push_str(&format!("# Activity Report — {}\n\n", report.period_label));
    out.push_str(&format!("**Period:** {} to {}  \n", report.from, report.to));
    out.push_str(&format!("**Total commits:** {}  \n", report.total_commits));
    out.push_str(&format!("**Total days:** {}  \n\n", report.total_days));

    // Heatmap as code block
    out.push_str("## Activity Heatmap\n\n");
    out.push_str("```\n");
    out.push_str(&render_heatmap_markdown(report));
    out.push_str("```\n\n");

    // Top weekdays
    out.push_str("## Top Active Days of Week\n\n");
    out.push_str("| Day | Commits |\n");
    out.push_str("| --- | ---: |\n");
    for (day, count) in &report.top_weekdays {
        out.push_str(&format!("| {} | {} |\n", day, count));
    }
    out.push('\n');

    // Statistics
    out.push_str("## Statistics\n\n");
    out.push_str(&format!(
        "- **Avg files per commit:** {:.1}\n",
        report.avg_files_per_commit
    ));
    out.push_str(&format!(
        "- **Avg commits per day:** {:.1}\n\n",
        report.avg_commits_per_day
    ));

    // Top repos
    out.push_str("## Top Repositories\n\n");
    out.push_str("| Repository | Commits |\n");
    out.push_str("| --- | ---: |\n");
    for (repo, count) in &report.top_repos {
        out.push_str(&format!(
            "| {} | {} |\n",
            escape_markdown_table_cell(repo),
            count
        ));
    }
    out.push('\n');

    out
}

pub fn export_markdown(report: &ActivityReport) -> Result<PathBuf, std::io::Error> {
    export_markdown_to_dir(report, Path::new("."))
}

fn export_markdown_to_dir(
    report: &ActivityReport,
    directory: &Path,
) -> Result<PathBuf, std::io::Error> {
    let today = Local::now().date_naive();
    let path = unique_export_path(&directory.join(format!(
        "diddo_activity_{}_month_{}.md",
        report.period_months, today
    )));
    let content = render_markdown(report);
    fs::write(&path, content)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone, Utc};

    use super::*;
    use crate::db::Commit;

    fn make_commit(repo: &str, date: NaiveDate, files: i64) -> Commit {
        let committed_at = Utc
            .with_ymd_and_hms(date.year(), date.month(), date.day(), 12, 0, 0)
            .unwrap();
        Commit {
            id: None,
            hash: format!("hash_{}", date),
            message: "test commit".to_string(),
            repo_path: format!("/tmp/{repo}"),
            repo_name: repo.to_string(),
            branch: "main".to_string(),
            files_changed: files,
            insertions: 10,
            deletions: 5,
            committed_at,
            author_email: None,
        }
    }

    fn make_commits_on_date(repo: &str, date: NaiveDate, count: usize) -> Vec<Commit> {
        (0..count)
            .map(|i| {
                let committed_at = Utc
                    .with_ymd_and_hms(date.year(), date.month(), date.day(), 9 + i as u32, 0, 0)
                    .unwrap();
                Commit {
                    id: None,
                    hash: format!("hash_{}_{}", date, i),
                    message: format!("commit {i}"),
                    repo_path: format!("/tmp/{repo}"),
                    repo_name: repo.to_string(),
                    branch: "main".to_string(),
                    files_changed: 3,
                    insertions: 10,
                    deletions: 5,
                    committed_at,
                    author_email: None,
                }
            })
            .collect()
    }

    #[test]
    fn build_report_empty_commits() {
        let from = NaiveDate::from_ymd_opt(2025, 4, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        let report = build_report(&[], from, to, 12);

        assert_eq!(report.total_commits, 0);
        assert_eq!(report.avg_files_per_commit, 0.0);
        assert_eq!(report.avg_commits_per_day, 0.0);
        assert!(report.top_repos.is_empty());
        assert!(report.heatmap.is_empty());
    }

    #[test]
    fn build_report_computes_stats_correctly() {
        let date1 = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(); // Tuesday
        let date2 = NaiveDate::from_ymd_opt(2026, 3, 11).unwrap(); // Wednesday
        let date3 = NaiveDate::from_ymd_opt(2026, 3, 12).unwrap(); // Thursday

        let commits = vec![
            make_commit("alpha", date1, 5),
            make_commit("alpha", date2, 3),
            make_commit("beta", date2, 7),
            make_commit("alpha", date3, 1),
        ];

        let from = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
        let report = build_report(&commits, from, to, 1);

        assert_eq!(report.total_commits, 4);
        assert_eq!(report.total_days, 30);

        // Avg files: (5+3+7+1)/4 = 4.0
        assert!((report.avg_files_per_commit - 4.0).abs() < 0.01);

        // Avg commits per day: 4/30
        assert!((report.avg_commits_per_day - 4.0 / 30.0).abs() < 0.01);

        // Top repos: alpha=3, beta=1
        assert_eq!(report.top_repos[0], ("alpha".to_string(), 3));
        assert_eq!(report.top_repos[1], ("beta".to_string(), 1));

        // Top weekdays: Wed=2, Tue=1, Thu=1
        assert_eq!(report.top_weekdays[0].0, "Wed");
        assert_eq!(report.top_weekdays[0].1, 2);
    }

    #[test]
    fn build_report_limits_top3_repos() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let mut commits = Vec::new();
        for (i, repo) in ["a", "b", "c", "d"].iter().enumerate() {
            for _ in 0..=(i + 1) {
                commits.push(make_commit(repo, date, 2));
            }
        }

        let from = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
        let report = build_report(&commits, from, to, 1);

        assert_eq!(report.top_repos.len(), 3);
        assert_eq!(report.top_repos[0].0, "d");
        assert_eq!(report.top_repos[1].0, "c");
        assert_eq!(report.top_repos[2].0, "b");
    }

    #[test]
    fn heatmap_has_correct_counts() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let commits = make_commits_on_date("repo", date, 5);

        let from = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
        let report = build_report(&commits, from, to, 1);

        assert_eq!(report.heatmap.get(&date), Some(&5));
    }

    #[test]
    fn heatmap_char_levels() {
        assert_eq!(heatmap_char(0), '·');
        assert_eq!(heatmap_char(1), '░');
        assert_eq!(heatmap_char(2), '░');
        assert_eq!(heatmap_char(3), '▒');
        assert_eq!(heatmap_char(5), '▒');
        assert_eq!(heatmap_char(6), '▓');
        assert_eq!(heatmap_char(9), '▓');
        assert_eq!(heatmap_char(10), '█');
        assert_eq!(heatmap_char(100), '█');
    }

    #[test]
    fn render_terminal_contains_all_sections() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let commits = vec![make_commit("my-repo", date, 4)];
        let from = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
        let report = build_report(&commits, from, to, 1);
        let rendered = render_terminal(&report);

        assert!(rendered.contains("Activity Report"));
        assert!(rendered.contains("1 commits over 30 days"));
        assert!(rendered.contains("Top active days of week"));
        assert!(rendered.contains("Statistics"));
        assert!(rendered.contains("Avg files per commit"));
        assert!(rendered.contains("Avg commits per day"));
        assert!(rendered.contains("Top repositories"));
        assert!(rendered.contains("my-repo"));
        // Heatmap legend (with ANSI color codes)
        assert!(rendered.contains("none"));
        assert!(rendered.contains("10+"));
    }

    #[test]
    fn render_markdown_escapes_pipe_in_repository_name() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let mut c = make_commit("a|b", date, 4);
        c.repo_name = "proj|wiki".to_string();
        let commits = vec![c];
        let from = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
        let report = build_report(&commits, from, to, 1);
        let rendered = render_markdown(&report);

        assert!(rendered.contains("| proj\\|wiki |"));
    }

    #[test]
    fn render_markdown_contains_all_sections() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let commits = vec![make_commit("my-repo", date, 4)];
        let from = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
        let report = build_report(&commits, from, to, 1);
        let rendered = render_markdown(&report);

        assert!(rendered.contains("# Activity Report"));
        assert!(rendered.contains("**Period:**"));
        assert!(rendered.contains("## Activity Heatmap"));
        assert!(rendered.contains("## Top Active Days of Week"));
        assert!(rendered.contains("| Day | Commits |"));
        assert!(rendered.contains("## Statistics"));
        assert!(rendered.contains("## Top Repositories"));
        assert!(rendered.contains("| my-repo |"));
    }

    #[test]
    fn compute_period_range_12_months() {
        let today = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        let (from, to) = compute_period_range(12, today);

        assert_eq!(from, NaiveDate::from_ymd_opt(2025, 4, 5).unwrap());
        assert_eq!(to, today);
    }

    #[test]
    fn compute_period_range_1_month() {
        let today = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        let (from, to) = compute_period_range(1, today);

        assert_eq!(from, NaiveDate::from_ymd_opt(2026, 3, 5).unwrap());
        assert_eq!(to, today);
    }

    #[test]
    fn shift_months_back_handles_month_boundary() {
        // March 31 - 1 month = Feb 28 (non-leap year)
        let date = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
        let shifted = shift_months_back(date, 1);
        assert_eq!(shifted, NaiveDate::from_ymd_opt(2026, 2, 28).unwrap());
    }

    #[test]
    fn shift_months_back_handles_year_boundary() {
        let date = NaiveDate::from_ymd_opt(2026, 2, 15).unwrap();
        let shifted = shift_months_back(date, 14);
        assert_eq!(shifted, NaiveDate::from_ymd_opt(2024, 12, 15).unwrap());
    }

    #[test]
    fn prev_monday_on_monday_returns_same() {
        let monday = NaiveDate::from_ymd_opt(2026, 3, 9).unwrap(); // Monday
        assert_eq!(prev_monday(monday), monday);
    }

    #[test]
    fn prev_monday_on_wednesday_returns_preceding_monday() {
        let wednesday = NaiveDate::from_ymd_opt(2026, 3, 11).unwrap(); // Wednesday
        assert_eq!(
            prev_monday(wednesday),
            NaiveDate::from_ymd_opt(2026, 3, 9).unwrap()
        );
    }

    #[test]
    fn next_sunday_on_sunday_returns_same() {
        let sunday = NaiveDate::from_ymd_opt(2026, 3, 15).unwrap(); // Sunday
        assert_eq!(next_sunday(sunday), sunday);
    }

    #[test]
    fn next_sunday_on_wednesday_returns_following_sunday() {
        let wednesday = NaiveDate::from_ymd_opt(2026, 3, 11).unwrap(); // Wednesday
        assert_eq!(
            next_sunday(wednesday),
            NaiveDate::from_ymd_opt(2026, 3, 15).unwrap()
        );
    }

    #[test]
    fn export_markdown_writes_file_with_correct_name_and_content() {
        let tmp = std::env::temp_dir().join(format!("diddo-export-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let date = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let commits = vec![make_commit("my-repo", date, 4)];
        let from = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
        let report = build_report(&commits, from, to, 1);

        let path = export_markdown_to_dir(&report, &tmp).unwrap();

        let today = chrono::Local::now().date_naive();
        let expected_name = format!("diddo_activity_1_month_{today}.md");
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), expected_name);

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Activity Report"));
        assert!(content.contains("## Top Repositories"));

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn export_markdown_avoids_overwriting_existing_file() {
        let tmp =
            std::env::temp_dir().join(format!("diddo-export-collision-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let date = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let commits = vec![make_commit("my-repo", date, 4)];
        let from = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
        let report = build_report(&commits, from, to, 1);

        let today = chrono::Local::now().date_naive();
        let first_name = tmp.join(format!("diddo_activity_1_month_{today}.md"));
        std::fs::write(&first_name, "existing").unwrap();

        let path = export_markdown_to_dir(&report, &tmp).unwrap();

        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            format!("diddo_activity_1_month_{today}_2.md")
        );
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# Activity Report"));

        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
