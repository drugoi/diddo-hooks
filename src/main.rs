mod ai;
mod config;
mod db;
mod hook;
mod init;
mod paths;
mod render;

use clap::{ArgGroup, Args, Parser, Subcommand};
use chrono::{Datelike, Duration, Local, NaiveDate};
use sha2::{Digest, Sha256};
use std::{cmp::Reverse, collections::BTreeMap, error::Error, ffi::OsString};

#[derive(Parser, Debug)]
#[command(
    name = "diddo",
    about = "Track your git commits, get AI-powered daily summaries",
    version
)]
struct HelpCli {
    #[command(subcommand)]
    command: Option<Commands>,
    #[command(flatten)]
    summary: SummaryArgs,
}

#[derive(Parser, Debug)]
#[command(
    name = "diddo",
    about = "Track your git commits, get AI-powered daily summaries",
    version
)]
struct CommandCli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Parser, Debug)]
#[command(
    name = "diddo",
    about = "Track your git commits, get AI-powered daily summaries",
    version
)]
struct TodayCli {
    #[command(flatten)]
    summary: SummaryArgs,
}

#[derive(Args, Debug, Default, Clone, Copy, PartialEq, Eq)]
#[command(group(
    ArgGroup::new("output")
        .args(["md", "raw", "json"])
        .multiple(false)
))]
struct SummaryArgs {
    /// Output as markdown.
    #[arg(long)]
    md: bool,

    /// Skip AI and show grouped raw commits.
    #[arg(long)]
    raw: bool,

    /// Output as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
enum Commands {
    /// Show today's summary.
    Today(SummaryArgs),
    /// Show yesterday's summary.
    Yesterday(SummaryArgs),
    /// Show this week's summary.
    Week(SummaryArgs),
    /// Install the global post-commit hook.
    Init,
    /// Remove the global hook and clean up.
    Uninstall,
    /// Internal hook entrypoint.
    Hook,
    /// Show the config location.
    Config,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ParsedCli {
    command: Option<Commands>,
    summary: SummaryArgs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SummaryPeriod {
    Today,
    Yesterday,
    Week,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Terminal,
    Markdown,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SummaryWindow {
    from: NaiveDate,
    to: NaiveDate,
    date_label: String,
    ai_period: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AiSummaryAttempt {
    summary: Option<String>,
    warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedSummary {
    output: String,
    warning: Option<String>,
}

fn parse_cli<I, T>(args: I) -> Result<ParsedCli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let second = args.get(1).and_then(|arg| arg.to_str());

    if matches!(second, Some("-h" | "--help" | "-V" | "--version")) {
        return HelpCli::try_parse_from(args).map(|cli| ParsedCli {
            command: cli.command,
            summary: cli.summary,
        });
    }

    if second.is_none() || second.is_some_and(|arg| arg.starts_with('-')) {
        return TodayCli::try_parse_from(args).map(|cli| ParsedCli {
            command: None,
            summary: cli.summary,
        });
    }

    CommandCli::try_parse_from(args).map(|cli| ParsedCli {
        command: cli.command,
        summary: SummaryArgs::default(),
    })
}

fn main() {
    let cli = parse_cli(std::env::args_os()).unwrap_or_else(|error| error.exit());

    let result = match cli.command {
        Some(Commands::Init) => run_init_command(),
        Some(Commands::Uninstall) => run_uninstall_command(),
        Some(Commands::Hook) => run_hook_command(),
        Some(Commands::Config) => run_config_command(),
        _ => run_summary_command(cli),
    };

    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_init_command() -> Result<(), Box<dyn Error>> {
    let paths = paths::AppPaths::new()?;
    init::install(&paths)?;
    Ok(())
}

fn run_uninstall_command() -> Result<(), Box<dyn Error>> {
    init::uninstall()?;
    Ok(())
}

fn run_hook_command() -> Result<(), Box<dyn Error>> {
    let paths = paths::AppPaths::new()?;
    let database = db::Database::open(&paths.db_path)?;

    hook::run(&database)?;

    Ok(())
}

fn run_config_command() -> Result<(), Box<dyn Error>> {
    let paths = paths::AppPaths::new()?;
    println!("{}", format_config_paths(&paths));

    Ok(())
}

fn format_config_paths(paths: &paths::AppPaths) -> String {
    format!(
        "Config file: {}\nDatabase path: {}\nHooks dir: {}",
        paths.config_path.display(),
        paths.db_path.display(),
        paths.hooks_dir.display()
    )
}

fn run_summary_command(cli: ParsedCli) -> Result<(), Box<dyn Error>> {
    let (period, summary_args) = summary_request_from_cli(cli)
        .ok_or_else(|| std::io::Error::other("summary command was not selected"))?;
    let paths = paths::AppPaths::new()?;
    let database = db::Database::open(&paths.db_path)?;
    let today = Local::now().date_naive();
    let window = resolve_summary_window(period, today);
    let commits = load_commits_for_window(&database, &window)?;
    let rendered = render_summary_output(
        summary_args,
        window,
        commits,
        || Ok(config::AppConfig::load(&paths.config_path)?),
        ai::create_provider,
    )?;

    if let Some(warning) = rendered.warning.as_deref() {
        eprintln!("{warning}");
    }
    print!("{}", rendered.output);

    Ok(())
}

fn summary_request_from_cli(cli: ParsedCli) -> Option<(SummaryPeriod, SummaryArgs)> {
    match cli.command {
        None => Some((SummaryPeriod::Today, cli.summary)),
        Some(Commands::Today(summary)) => Some((SummaryPeriod::Today, summary)),
        Some(Commands::Yesterday(summary)) => Some((SummaryPeriod::Yesterday, summary)),
        Some(Commands::Week(summary)) => Some((SummaryPeriod::Week, summary)),
        Some(Commands::Init | Commands::Uninstall | Commands::Hook | Commands::Config) => None,
    }
}

fn resolve_summary_window(period: SummaryPeriod, today: NaiveDate) -> SummaryWindow {
    match period {
        SummaryPeriod::Today => SummaryWindow {
            from: today,
            to: today,
            date_label: format!("{today} (today)"),
            ai_period: "today",
        },
        SummaryPeriod::Yesterday => {
            let yesterday = today - Duration::days(1);
            SummaryWindow {
                from: yesterday,
                to: yesterday,
                date_label: format!("{yesterday} (yesterday)"),
                ai_period: "yesterday",
            }
        }
        SummaryPeriod::Week => {
            let week_start = today - Duration::days(today.weekday().num_days_from_monday().into());
            SummaryWindow {
                from: week_start,
                to: today,
                date_label: format!("{week_start} to {today} (week)"),
                ai_period: "this week",
            }
        }
    }
}

fn load_commits_for_window(
    database: &db::Database,
    window: &SummaryWindow,
) -> Result<Vec<db::Commit>, Box<dyn Error>> {
    if window.from == window.to {
        return Ok(database.query_date(window.from)?);
    }

    Ok(database.query_date_range(window.from, window.to)?)
}

fn should_try_ai_summary(summary_args: SummaryArgs) -> bool {
    !summary_args.raw
}

fn compute_cache_key(provider_id: &str, model_id: &str, period: &str, prompt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(provider_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(model_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(period.as_bytes());
    hasher.update(b"\0");
    hasher.update(prompt.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn output_format(summary_args: SummaryArgs) -> OutputFormat {
    if summary_args.md {
        OutputFormat::Markdown
    } else if summary_args.json {
        OutputFormat::Json
    } else {
        OutputFormat::Terminal
    }
}

fn try_ai_summary<F>(
    ai_config: &config::AiConfig,
    commits: &[db::Commit],
    period: &str,
    allow_ai: bool,
    create_provider: F,
) -> AiSummaryAttempt
where
    F: FnOnce(&config::AiConfig) -> ai::Result<Box<dyn ai::AiProvider>>,
{
    if !allow_ai {
        return AiSummaryAttempt {
            summary: None,
            warning: None,
        };
    }

    let provider = match create_provider(ai_config) {
        Ok(provider) => provider,
        Err(error) => {
            return AiSummaryAttempt {
                summary: None,
                warning: Some(format!(
                    "AI summary unavailable: {error}. Falling back to raw output."
                )),
            };
        }
    };

    match provider.summarize(commits, period) {
        Ok(summary) => AiSummaryAttempt {
            summary: Some(summary),
            warning: None,
        },
        Err(error) => AiSummaryAttempt {
            summary: None,
            warning: Some(format!(
                "AI summary failed: {error}. Falling back to raw output."
            )),
        },
    }
}

fn render_summary_output<FConfig, FProvider>(
    summary_args: SummaryArgs,
    window: SummaryWindow,
    commits: Vec<db::Commit>,
    load_config: FConfig,
    create_provider: FProvider,
) -> Result<RenderedSummary, Box<dyn Error>>
where
    FConfig: FnOnce() -> Result<config::AppConfig, Box<dyn Error>>,
    FProvider: FnOnce(&config::AiConfig) -> ai::Result<Box<dyn ai::AiProvider>>,
{
    if commits.is_empty() {
        return Ok(RenderedSummary {
            output: render_empty_summary(&window.date_label, summary_args),
            warning: None,
        });
    }

    let ai_attempt = if should_try_ai_summary(summary_args) {
        let config = load_config()?;
        try_ai_summary(&config.ai, &commits, window.ai_period, true, create_provider)
    } else {
        AiSummaryAttempt {
            summary: None,
            warning: None,
        }
    };

    let summary = build_summary_data(window.date_label, ai_attempt.summary, commits);
    let output = match output_format(summary_args) {
        OutputFormat::Terminal => render::render_terminal_to_string(&summary),
        OutputFormat::Markdown => render::render_markdown(&summary),
        OutputFormat::Json => render::render_json(&summary),
    };

    Ok(RenderedSummary {
        output,
        warning: ai_attempt.warning,
    })
}

fn build_summary_data(
    date_label: String,
    ai_summary: Option<String>,
    commits: Vec<db::Commit>,
) -> render::SummaryData {
    let mut project_counts = BTreeMap::<(String, String), usize>::new();

    for commit in &commits {
        *project_counts
            .entry((commit.repo_name.clone(), commit.repo_path.clone()))
            .or_default() += 1;
    }

    let mut ranked_projects = project_counts
        .into_iter()
        .map(|((repo_name, repo_path), count)| (repo_name, repo_path, count))
        .collect::<Vec<_>>();
    ranked_projects.sort_by(|left, right| {
        (Reverse(left.2), left.0.as_str(), left.1.as_str()).cmp(&(
            Reverse(right.2),
            right.0.as_str(),
            right.1.as_str(),
        ))
    });

    let first_commit = commits.iter().min_by_key(|commit| commit.committed_at);
    let last_commit = commits.iter().max_by_key(|commit| commit.committed_at);
    let most_active = ranked_projects.first();

    render::SummaryData {
        date_label,
        ai_summary,
        total_commits: commits.len(),
        project_count: ranked_projects.len(),
        first_commit_time: first_commit
            .map(|commit| format_commit_time(commit.committed_at))
            .unwrap_or_else(|| "-".to_string()),
        last_commit_time: last_commit
            .map(|commit| format_commit_time(commit.committed_at))
            .unwrap_or_else(|| "-".to_string()),
        most_active_project: most_active
            .map(|(repo_name, _, _)| repo_name.clone())
            .unwrap_or_else(|| "-".to_string()),
        most_active_count: most_active.map(|(_, _, count)| *count).unwrap_or(0),
        commits,
    }
}

fn format_commit_time(committed_at: chrono::DateTime<chrono::Utc>) -> String {
    committed_at
        .with_timezone(&Local)
        .format("%H:%M")
        .to_string()
}

fn render_empty_summary(date_label: &str, summary_args: SummaryArgs) -> String {
    let message = format!("No commits recorded for {date_label}.");

    match output_format(summary_args) {
        OutputFormat::Terminal => message,
        OutputFormat::Markdown => format!("# {date_label}\n\n{message}\n"),
        OutputFormat::Json => render::render_json(&build_summary_data(
            date_label.to_string(),
            None,
            Vec::new(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::{NaiveDate, TimeZone, Utc};

    use super::{
        AiSummaryAttempt, Commands, OutputFormat, ParsedCli, SummaryArgs, SummaryPeriod,
        build_summary_data, format_commit_time, format_config_paths, output_format, parse_cli,
        render_empty_summary, render_summary_output, resolve_summary_window,
        should_try_ai_summary, summary_request_from_cli, try_ai_summary,
    };
    use crate::{
        ai::{AiError, AiProvider},
        paths::AppPaths,
    };

    #[test]
    fn parses_one_summary_output_flag_for_today_and_subcommands() {
        let today = parse_cli(["diddo", "--md"]).unwrap();
        let explicit_today = parse_cli(["diddo", "today", "--raw"]).unwrap();
        let yesterday = parse_cli(["diddo", "yesterday", "--json"]).unwrap();
        let week = parse_cli(["diddo", "week", "--raw"]).unwrap();

        assert_eq!(
            today,
            ParsedCli {
                command: None,
                summary: SummaryArgs {
                    md: true,
                    raw: false,
                    json: false,
                },
            }
        );
        assert_eq!(
            explicit_today.command,
            Some(Commands::Today(super::SummaryArgs {
                md: false,
                raw: true,
                json: false,
            }))
        );
        assert_eq!(
            yesterday.command,
            Some(Commands::Yesterday(super::SummaryArgs {
                md: false,
                raw: false,
                json: true,
            }))
        );
        assert_eq!(
            week.command,
            Some(Commands::Week(super::SummaryArgs {
                md: false,
                raw: true,
                json: false,
            }))
        );
    }

    #[test]
    fn parses_default_top_level_summary_without_flags() {
        let cli = parse_cli(["diddo"]).unwrap();

        assert_eq!(
            cli,
            ParsedCli {
                command: None,
                summary: SummaryArgs::default(),
            }
        );
    }

    #[test]
    fn supports_long_version_flag() {
        let error = parse_cli(["diddo", "--version"]).unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    #[test]
    fn supports_short_version_flag() {
        let error = parse_cli(["diddo", "-V"]).unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    #[test]
    fn rejects_conflicting_summary_output_flags() {
        let error = parse_cli(["diddo", "--md", "--json"]).unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn rejects_summary_output_flags_on_non_summary_commands() {
        let error = parse_cli(["diddo", "init", "--json"]).unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn rejects_pre_subcommand_summary_flags_on_non_summary_commands() {
        let error = parse_cli(["diddo", "--json", "init"]).unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn derives_summary_request_from_default_and_subcommand_forms() {
        let today = summary_request_from_cli(parse_cli(["diddo"]).unwrap());
        let explicit_today = summary_request_from_cli(parse_cli(["diddo", "today"]).unwrap());
        let yesterday = summary_request_from_cli(parse_cli(["diddo", "yesterday"]).unwrap());
        let week = summary_request_from_cli(parse_cli(["diddo", "week", "--md"]).unwrap());
        let init = summary_request_from_cli(parse_cli(["diddo", "init"]).unwrap());

        assert_eq!(today, Some((SummaryPeriod::Today, SummaryArgs::default())));
        assert_eq!(
            explicit_today,
            Some((SummaryPeriod::Today, SummaryArgs::default()))
        );
        assert_eq!(
            yesterday,
            Some((SummaryPeriod::Yesterday, SummaryArgs::default()))
        );
        assert_eq!(
            week,
            Some((
                SummaryPeriod::Week,
                SummaryArgs {
                    md: true,
                    raw: false,
                    json: false,
                }
            ))
        );
        assert_eq!(init, None);
    }

    #[test]
    fn resolves_week_window_from_monday_through_today() {
        let today = NaiveDate::from_ymd_opt(2026, 3, 12).unwrap();

        let window = resolve_summary_window(SummaryPeriod::Week, today);

        assert_eq!(window.from, NaiveDate::from_ymd_opt(2026, 3, 9).unwrap());
        assert_eq!(window.to, today);
        assert_eq!(window.date_label, "2026-03-09 to 2026-03-12 (week)");
        assert_eq!(window.ai_period, "this week");
    }

    #[test]
    fn disables_ai_for_raw_and_json_outputs() {
        assert!(should_try_ai_summary(SummaryArgs::default()));
        assert!(should_try_ai_summary(SummaryArgs {
            md: true,
            raw: false,
            json: false,
        }));
        assert!(should_try_ai_summary(SummaryArgs {
            md: false,
            raw: false,
            json: true,
        }));
        assert!(!should_try_ai_summary(SummaryArgs {
            md: false,
            raw: true,
            json: false,
        }));
    }

    #[test]
    fn ai_attempt_keeps_json_mode_eligible_for_ai_summary() {
        let commits = vec![sample_commit("aaa1111", "diddo", "/tmp/diddo", 9, 15)];

        let attempt = try_ai_summary(
            &crate::config::AiConfig::default(),
            &commits,
            "today",
            should_try_ai_summary(SummaryArgs {
                md: false,
                raw: false,
                json: true,
            }),
            |_config| Ok(Box::new(SuccessProvider("JSON summary".to_string()))),
        );

        assert_eq!(
            attempt,
            AiSummaryAttempt {
                summary: Some("JSON summary".to_string()),
                warning: None,
            }
        );
    }

    #[test]
    fn ai_attempt_surfaces_warning_when_provider_is_unavailable() {
        let commits = vec![sample_commit("aaa1111", "diddo", "/tmp/diddo", 9, 15)];

        let attempt = try_ai_summary(
            &crate::config::AiConfig::default(),
            &commits,
            "today",
            true,
            |_config| Err(AiError::new("no AI provider configured or detected")),
        );

        assert_eq!(attempt.summary, None);
        assert_eq!(
            attempt.warning.as_deref(),
            Some("AI summary unavailable: no AI provider configured or detected. Falling back to raw output.")
        );
    }

    #[test]
    fn ai_attempt_surfaces_warning_when_provider_fails_at_runtime() {
        let commits = vec![sample_commit("aaa1111", "diddo", "/tmp/diddo", 9, 15)];

        let attempt = try_ai_summary(
            &crate::config::AiConfig::default(),
            &commits,
            "today",
            true,
            |_config| Ok(Box::new(FailingProvider("claude failed".to_string()))),
        );

        assert_eq!(attempt.summary, None);
        assert_eq!(
            attempt.warning.as_deref(),
            Some("AI summary failed: claude failed. Falling back to raw output.")
        );
    }

    #[test]
    fn maps_summary_flags_to_expected_output_formats() {
        assert_eq!(output_format(SummaryArgs::default()), OutputFormat::Terminal);
        assert_eq!(
            output_format(SummaryArgs {
                md: true,
                raw: false,
                json: false,
            }),
            OutputFormat::Markdown
        );
        assert_eq!(
            output_format(SummaryArgs {
                md: false,
                raw: false,
                json: true,
            }),
            OutputFormat::Json
        );
    }

    #[test]
    fn builds_deterministic_summary_stats_from_commits() {
        let first_commit = sample_commit("aaa1111", "diddo", "/tmp/diddo", 9, 15);
        let second_commit = sample_commit("ccc3333", "diddo", "/tmp/diddo", 10, 45);
        let last_commit = sample_commit("bbb2222", "api-service", "/tmp/api-service", 15, 20);
        let summary = build_summary_data(
            "2026-03-10 (today)".to_string(),
            Some("Shipped summary flow.".to_string()),
            vec![
                last_commit.clone(),
                first_commit.clone(),
                second_commit,
            ],
        );

        assert_eq!(summary.total_commits, 3);
        assert_eq!(summary.project_count, 2);
        assert_eq!(summary.first_commit_time, format_commit_time(first_commit.committed_at));
        assert_eq!(summary.last_commit_time, format_commit_time(last_commit.committed_at));
        assert_eq!(summary.most_active_project, "diddo");
        assert_eq!(summary.most_active_count, 2);
        assert_eq!(summary.ai_summary.as_deref(), Some("Shipped summary flow."));
    }

    #[test]
    fn renders_useful_empty_period_messages_for_all_output_formats() {
        let terminal = render_empty_summary("2026-03-10 (today)", SummaryArgs::default());
        let markdown = render_empty_summary(
            "2026-03-10 (today)",
            SummaryArgs {
                md: true,
                raw: false,
                json: false,
            },
        );
        let json = render_empty_summary(
            "2026-03-10 (today)",
            SummaryArgs {
                md: false,
                raw: false,
                json: true,
            },
        );

        assert_eq!(terminal, "No commits recorded for 2026-03-10 (today).");
        assert_eq!(
            markdown,
            "# 2026-03-10 (today)\n\nNo commits recorded for 2026-03-10 (today).\n"
        );
        assert!(json.contains("\"date_label\": \"2026-03-10 (today)\""));
        assert!(json.contains("\"projects\": []"));
        assert!(json.contains("\"total_commits\": 0"));
        assert!(json.contains("\"project_count\": 0"));
        assert!(json.contains("\"first_commit_time\": \"-\""));
        assert!(json.contains("\"last_commit_time\": \"-\""));
        assert!(json.contains("\"most_active_project\": \"-\""));
        assert!(json.contains("\"most_active_count\": 0"));
        assert!(!json.contains("\"message\""));
    }

    #[test]
    fn empty_summary_output_does_not_depend_on_config_loading() {
        let window = resolve_summary_window(
            SummaryPeriod::Today,
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
        );

        let rendered = render_summary_output(
            SummaryArgs {
                md: false,
                raw: false,
                json: true,
            },
            window,
            Vec::new(),
            || Err(std::io::Error::other("bad config").into()),
            |_config| Ok(Box::new(SuccessProvider("unused".to_string()))),
        )
        .unwrap();

        assert_eq!(rendered.warning, None);
        assert!(rendered.output.contains("\"date_label\": \"2026-03-10 (today)\""));
        assert!(rendered.output.contains("\"projects\": []"));
        assert!(rendered.output.contains("\"ai_summary\": null"));
        assert!(!rendered.output.contains("\"message\""));
    }

    #[test]
    fn formats_all_paths_for_the_config_command() {
        let paths = AppPaths {
            db_path: PathBuf::from("/tmp/diddo/commits.db"),
            config_path: PathBuf::from("/tmp/diddo/config.toml"),
            hooks_dir: PathBuf::from("/tmp/diddo/hooks"),
        };

        assert_eq!(
            format_config_paths(&paths),
            "Config file: /tmp/diddo/config.toml\nDatabase path: /tmp/diddo/commits.db\nHooks dir: /tmp/diddo/hooks"
        );
    }

    fn sample_commit(
        hash: &str,
        repo_name: &str,
        repo_path: &str,
        hour: u32,
        minute: u32,
    ) -> crate::db::Commit {
        crate::db::Commit {
            id: None,
            hash: hash.to_string(),
            message: format!("feat: update {repo_name}"),
            repo_path: repo_path.to_string(),
            repo_name: repo_name.to_string(),
            branch: "main".to_string(),
            files_changed: 3,
            insertions: 12,
            deletions: 4,
            committed_at: Utc.with_ymd_and_hms(2026, 3, 10, hour, minute, 0).unwrap(),
        }
    }

    struct SuccessProvider(String);

    impl AiProvider for SuccessProvider {
        fn summarize(
            &self,
            _commits: &[crate::db::Commit],
            _period: &str,
        ) -> crate::ai::Result<String> {
            Ok(self.0.clone())
        }
    }

    struct FailingProvider(String);

    impl AiProvider for FailingProvider {
        fn summarize(
            &self,
            _commits: &[crate::db::Commit],
            _period: &str,
        ) -> crate::ai::Result<String> {
            Err(AiError::new(self.0.clone()))
        }
    }
}
