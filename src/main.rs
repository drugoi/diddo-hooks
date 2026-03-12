mod ai;
mod config;
mod db;
mod hook;
mod init;
mod interactive;
mod paths;
mod render;
mod summary_group;
mod update;

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Utc};
use clap::{ArgGroup, Args, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::{
    cmp::Reverse, collections::BTreeMap, error::Error, ffi::OsString, io::IsTerminal,
    time::Duration as StdDuration,
};

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
        .args(["md", "raw", "json", "table"])
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

    /// Output repository activity as a table.
    #[arg(long)]
    table: bool,

    /// Skip cache and force a fresh AI summary.
    #[arg(long)]
    no_cache: bool,
}

#[derive(Args, Debug, Clone, Copy, PartialEq, Eq)]
struct UpdateArgs {
    /// Apply update without prompting.
    #[arg(long, alias = "assume-yes")]
    yes: bool,
}

#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
enum Commands {
    /// Show today's summary.
    Today(SummaryArgs),
    /// Show yesterday's summary.
    Yesterday(SummaryArgs),
    /// Show this week's summary.
    Week(SummaryArgs),
    /// Show summary for the last 24 hours.
    Standup(SummaryArgs),
    /// Install the global post-commit hook.
    Init,
    /// Remove the global hook and clean up.
    Uninstall,
    /// Internal hook entrypoint.
    Hook,
    /// Show the config location.
    Config,
    /// Show database metadata.
    Metadata,
    /// Update diddo to the latest release.
    Update(UpdateArgs),
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
    Standup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Terminal,
    Markdown,
    Json,
    Table,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SummaryWindow {
    from: NaiveDate,
    to: NaiveDate,
    date_label: String,
    ai_period: &'static str,
    exact_bounds: Option<(DateTime<Utc>, DateTime<Utc>)>,
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
    let raw_args: Vec<OsString> = std::env::args_os().collect();
    let is_bare_invocation = raw_args.len() == 1
        || raw_args.iter().skip(1).all(|arg| {
            arg.to_str().is_some_and(|s| {
                s.starts_with('-') && !matches!(s, "-h" | "--help" | "-V" | "--version")
            })
        });

    if is_bare_invocation && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        let selected = match interactive::run() {
            Ok(Some(command)) => command,
            Ok(None) => return,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };
        let cli = parse_cli(["diddo", selected.as_str()]).unwrap_or_else(|error| error.exit());
        run_cli(cli);
        return;
    }

    let cli = parse_cli(raw_args).unwrap_or_else(|error| error.exit());
    run_cli(cli);
}

fn run_cli(cli: ParsedCli) {
    let result = match cli.command {
        Some(Commands::Init) => run_init_command(),
        Some(Commands::Uninstall) => run_uninstall_command(),
        Some(Commands::Hook) => run_hook_command(),
        Some(Commands::Config) => run_config_command(),
        Some(Commands::Metadata) => run_metadata_command(),
        Some(Commands::Update(args)) => run_update_command(args),
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

fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes == 0 {
        "0 bytes".to_string()
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    }
}

fn run_metadata_command() -> Result<(), Box<dyn Error>> {
    let paths = paths::AppPaths::new()?;
    let database = db::Database::open(&paths.db_path)?;
    let size_bytes = std::fs::metadata(&paths.db_path)?.len();

    println!("{}", format_metadata(&database, size_bytes)?);

    Ok(())
}

fn run_update_command(args: UpdateArgs) -> Result<(), Box<dyn Error>> {
    update::run(args.yes)
}

fn format_metadata(database: &db::Database, size_bytes: u64) -> Result<String, Box<dyn Error>> {
    let count = database.commit_count()?;
    let oldest = match database.oldest_commit_date()? {
        Some(raw) => chrono::DateTime::parse_from_rfc3339(&raw)
            .map(|dt| {
                dt.with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            })
            .unwrap_or(raw),
        None => "-".to_string(),
    };

    Ok(format!(
        "Database size:   {}\nTotal commits:   {count}\nOldest commit:   {oldest}",
        format_file_size(size_bytes)
    ))
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
        &database,
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
        Some(Commands::Standup(summary)) => Some((SummaryPeriod::Standup, summary)),
        Some(
            Commands::Init
            | Commands::Uninstall
            | Commands::Hook
            | Commands::Config
            | Commands::Metadata
            | Commands::Update(_),
        ) => None,
    }
}

fn resolve_summary_window(period: SummaryPeriod, today: NaiveDate) -> SummaryWindow {
    match period {
        SummaryPeriod::Today => SummaryWindow {
            from: today,
            to: today,
            date_label: format!("{today} (today)"),
            ai_period: "today",
            exact_bounds: None,
        },
        SummaryPeriod::Yesterday => {
            let yesterday = today - Duration::days(1);
            SummaryWindow {
                from: yesterday,
                to: yesterday,
                date_label: format!("{yesterday} (yesterday)"),
                ai_period: "yesterday",
                exact_bounds: None,
            }
        }
        SummaryPeriod::Week => {
            let week_start = today - Duration::days(today.weekday().num_days_from_monday().into());
            SummaryWindow {
                from: week_start,
                to: today,
                date_label: format!("{week_start} to {today} (week)"),
                ai_period: "this week",
                exact_bounds: None,
            }
        }
        SummaryPeriod::Standup => {
            let now = Local::now();
            let from = now - Duration::hours(24);
            SummaryWindow {
                from: from.date_naive(),
                to: today,
                date_label: "last 24 hours (standup)".to_string(),
                ai_period: "the last 24 hours",
                exact_bounds: Some((from.with_timezone(&Utc), now.with_timezone(&Utc))),
            }
        }
    }
}

fn load_commits_for_window(
    database: &db::Database,
    window: &SummaryWindow,
) -> Result<Vec<db::Commit>, Box<dyn Error>> {
    if let Some((from, to)) = window.exact_bounds {
        return Ok(database.query_datetime_range(from, to)?);
    }

    if window.from == window.to {
        return Ok(database.query_date(window.from)?);
    }

    Ok(database.query_date_range(window.from, window.to)?)
}

fn should_try_ai_summary(summary_args: SummaryArgs) -> bool {
    !summary_args.raw && !summary_args.table
}

fn compute_cache_key(
    provider_id: &str,
    model_id: &str,
    period: &str,
    profile: &str,
    prompt: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(provider_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(model_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(period.as_bytes());
    hasher.update(b"\0");
    hasher.update(profile.as_bytes());
    hasher.update(b"\0");
    hasher.update(prompt.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn output_format(summary_args: SummaryArgs) -> OutputFormat {
    if summary_args.md {
        OutputFormat::Markdown
    } else if summary_args.json {
        OutputFormat::Json
    } else if summary_args.table {
        OutputFormat::Table
    } else {
        OutputFormat::Terminal
    }
}

#[cfg(test)]
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
    database: &db::Database,
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

    let format = output_format(summary_args);

    if matches!(format, OutputFormat::Table) {
        return Ok(RenderedSummary {
            output: render::render_table(&commits, &window.date_label),
            warning: None,
        });
    }

    let mut groups = summary_group::group_commits_by_profile_then_repo(&commits);
    let period = window.ai_period;

    let (_, warning) = if should_try_ai_summary(summary_args) {
        let config = load_config()?;
        let instructions = config.ai.resolved_prompt_instructions();
        let provider_identity = ai::primary_provider_identity(&config.ai).ok();
        let provider = match create_provider(&config.ai) {
            Ok(p) => p,
            Err(error) => {
                let global_stats = build_global_stats(&commits);
                let output = match format {
                    OutputFormat::Terminal => render::render_terminal_to_string_by_profile(
                        &groups,
                        &window.date_label,
                        &global_stats,
                    ),
                    OutputFormat::Markdown => render::render_markdown_by_profile(
                        &groups,
                        &window.date_label,
                        &global_stats,
                    ),
                    OutputFormat::Json => {
                        render::render_json_by_profile(&groups, &window.date_label, &global_stats)
                    }
                    OutputFormat::Table => unreachable!("table mode returns early"),
                };
                return Ok(RenderedSummary {
                    output,
                    warning: Some(format!(
                        "AI summary unavailable: {error}. Falling back to raw output."
                    )),
                });
            }
        };
        let show_indicator = std::io::stderr().is_terminal();
        let mut warnings = Vec::new();

        for profile_group in groups.iter_mut() {
            let profile_commits: Vec<db::Commit> = profile_group
                .repos
                .iter()
                .flat_map(|r| r.commits.iter().cloned())
                .collect();
            let prompt = ai::build_prompt(&profile_commits, period, instructions);
            let cache_key_opt = provider_identity.as_ref().map(|(provider_id, model_id)| {
                compute_cache_key(
                    provider_id,
                    model_id,
                    period,
                    &profile_group.profile_label,
                    &prompt,
                )
            });
            let cached = match (cache_key_opt.as_deref(), summary_args.no_cache) {
                (Some(key), false) => database.get_cached_summary(key).ok().flatten(),
                _ => None,
            };
            let summary = if let Some(cached_summary) = cached {
                Some(cached_summary)
            } else {
                if show_indicator {
                    let pb = ProgressBar::new_spinner();
                    pb.set_style(
                        ProgressStyle::default_spinner()
                            .template("{spinner} {msg}")
                            .unwrap(),
                    );
                    pb.set_message("Generating AI summary...");
                    pb.enable_steady_tick(StdDuration::from_millis(80));
                    let attempt = match provider.summarize(&profile_commits, period) {
                        Ok(s) => AiSummaryAttempt {
                            summary: Some(s),
                            warning: None,
                        },
                        Err(error) => AiSummaryAttempt {
                            summary: None,
                            warning: Some(format!(
                                "AI summary failed: {error}. Falling back to raw output."
                            )),
                        },
                    };
                    pb.finish_and_clear();
                    if let Some(ref w) = attempt.warning {
                        warnings.push(w.clone());
                    }
                    if let (Some(ref key), Some(ref s)) =
                        (cache_key_opt.as_ref(), attempt.summary.as_ref())
                    {
                        let _ = database.set_cached_summary(key, s);
                    }
                    attempt.summary
                } else {
                    eprintln!("Generating AI summary...");
                    let attempt = match provider.summarize(&profile_commits, period) {
                        Ok(s) => AiSummaryAttempt {
                            summary: Some(s),
                            warning: None,
                        },
                        Err(error) => AiSummaryAttempt {
                            summary: None,
                            warning: Some(format!(
                                "AI summary failed: {error}. Falling back to raw output."
                            )),
                        },
                    };
                    if let Some(ref w) = attempt.warning {
                        warnings.push(w.clone());
                    }
                    if let (Some(ref key), Some(ref s)) =
                        (cache_key_opt.as_ref(), attempt.summary.as_ref())
                    {
                        let _ = database.set_cached_summary(key, s);
                    }
                    attempt.summary
                }
            };
            if let Some(s) = summary {
                profile_group.ai_summary = Some(s);
            } else {
                profile_group.ai_summary = None;
            }
        }

        let combined = groups
            .iter()
            .filter_map(|g| g.ai_summary.as_ref().map(String::as_str))
            .collect::<Vec<_>>()
            .join("\n\n");
        let combined_summary = if combined.is_empty() {
            None
        } else {
            Some(combined)
        };
        let warning = if warnings.is_empty() {
            None
        } else {
            Some(warnings.join("; "))
        };
        (combined_summary, warning)
    } else {
        (None, None)
    };

    let global_stats = build_global_stats(&commits);
    let include_table = !summary_args.raw;
    let output = match format {
        OutputFormat::Terminal => render::render_terminal_to_string_by_profile_with_table(
            &groups,
            &window.date_label,
            &global_stats,
            include_table,
        ),
        OutputFormat::Markdown => render::render_markdown_by_profile_with_table(
            &groups,
            &window.date_label,
            &global_stats,
            include_table,
        ),
        OutputFormat::Json => {
            render::render_json_by_profile(&groups, &window.date_label, &global_stats)
        }
        OutputFormat::Table => unreachable!("table mode returns early"),
    };

    Ok(RenderedSummary { output, warning })
}

fn build_global_stats(commits: &[db::Commit]) -> render::GlobalStats {
    let mut project_counts = BTreeMap::<(String, String), usize>::new();
    for commit in commits {
        *project_counts
            .entry((commit.repo_name.clone(), commit.repo_path.clone()))
            .or_default() += 1;
    }
    let mut ranked_projects = project_counts
        .into_iter()
        .map(|((repo_name, _repo_path), count)| (repo_name, count))
        .collect::<Vec<_>>();
    ranked_projects.sort_by(|left, right| {
        (Reverse(left.1), left.0.as_str()).cmp(&(Reverse(right.1), right.0.as_str()))
    });

    let first_commit = commits.iter().min_by_key(|c| c.committed_at);
    let last_commit = commits.iter().max_by_key(|c| c.committed_at);
    let most_active = ranked_projects.first();

    render::GlobalStats {
        total_commits: commits.len(),
        first_commit_time: first_commit
            .map(|c| format_commit_time(c.committed_at))
            .unwrap_or_else(|| "-".to_string()),
        last_commit_time: last_commit
            .map(|c| format_commit_time(c.committed_at))
            .unwrap_or_else(|| "-".to_string()),
        most_active_project: most_active
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "-".to_string()),
        most_active_count: most_active.map(|(_, count)| *count).unwrap_or(0),
    }
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
        OutputFormat::Table => message,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::PathBuf;

    use chrono::{NaiveDate, TimeZone, Utc};

    use super::{
        AiSummaryAttempt, Commands, OutputFormat, ParsedCli, SummaryArgs, SummaryPeriod,
        build_summary_data, compute_cache_key, format_commit_time, format_config_paths,
        format_file_size, format_metadata, output_format, parse_cli, render_empty_summary,
        render_summary_output, resolve_summary_window, should_try_ai_summary,
        summary_request_from_cli, try_ai_summary,
    };
    use crate::{
        ai::{AiError, AiProvider},
        config::{AiCliConfig, AiConfig, AppConfig},
        paths::AppPaths,
    };

    #[test]
    fn cache_key_is_deterministic_and_different_for_different_inputs() {
        let prompt = "Period: today\n\n1. [repo] fix: bug (abc) on main at 2026-03-10T12:00:00Z\n";
        let key1 = compute_cache_key("openai", "gpt-4o-mini", "today", "unknown", prompt);
        let key2 = compute_cache_key("openai", "gpt-4o-mini", "today", "unknown", prompt);
        assert_eq!(key1, key2);

        let key3 = compute_cache_key("anthropic", "gpt-4o-mini", "today", "unknown", prompt);
        assert_ne!(key1, key3);
        let key4 = compute_cache_key("openai", "other-model", "today", "unknown", prompt);
        assert_ne!(key1, key4);
        let key5 = compute_cache_key("openai", "gpt-4o-mini", "yesterday", "unknown", prompt);
        assert_ne!(key1, key5);
        let key6 = compute_cache_key(
            "openai",
            "gpt-4o-mini",
            "today",
            "unknown",
            "different prompt",
        );
        assert_ne!(key1, key6);
        let key7 = compute_cache_key("openai", "gpt-4o-mini", "today", "test@example.com", prompt);
        assert_ne!(key1, key7);
    }

    #[test]
    fn cache_hit_returns_stored_summary() {
        use crate::db::Database;

        let database = Database::open_in_memory().unwrap();
        let config = AppConfig {
            ai: AiConfig {
                provider: Some("openai".to_string()),
                api_key: Some("test-key".to_string()),
                model: Some("gpt-4o-mini".to_string()),
                cli: AiCliConfig {
                    prefer: Some("api".to_string()),
                },
                ..AiConfig::default()
            },
            ..AppConfig::default()
        };
        let commits = vec![sample_commit("abc123", "diddo", "/tmp/diddo", 9, 15)];
        let period = "today";
        let prompt =
            crate::ai::build_prompt(&commits, period, config.ai.resolved_prompt_instructions());
        let (provider_id, model_id) = crate::ai::primary_provider_identity(&config.ai).unwrap();
        let key = compute_cache_key(&provider_id, &model_id, period, "unknown", &prompt);
        database
            .set_cached_summary(&key, "Pre-cached summary")
            .unwrap();

        let window = resolve_summary_window(
            SummaryPeriod::Today,
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
        );
        let rendered = render_summary_output(
            &database,
            SummaryArgs {
                md: false,
                raw: false,
                json: true,
                table: false,
                no_cache: false,
            },
            window,
            commits,
            || Ok(config.clone()),
            |_| {
                Ok(Box::new(FailingProvider(
                    "should not be called".to_string(),
                )))
            },
        )
        .unwrap();

        assert!(rendered.output.contains("Pre-cached summary"));
        assert_eq!(rendered.warning, None);
    }

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
                    table: false,
                    no_cache: false,
                },
            }
        );
        assert_eq!(
            explicit_today.command,
            Some(Commands::Today(super::SummaryArgs {
                md: false,
                raw: true,
                json: false,
                table: false,
                no_cache: false,
            }))
        );
        assert_eq!(
            yesterday.command,
            Some(Commands::Yesterday(super::SummaryArgs {
                md: false,
                raw: false,
                json: true,
                table: false,
                no_cache: false,
            }))
        );
        assert_eq!(
            week.command,
            Some(Commands::Week(super::SummaryArgs {
                md: false,
                raw: true,
                json: false,
                table: false,
                no_cache: false,
            }))
        );
    }

    #[test]
    fn parses_table_output_flag_for_default_and_subcommands() {
        let default_today = parse_cli(["diddo", "--table"]).unwrap();
        let explicit_today = parse_cli(["diddo", "today", "--table"]).unwrap();
        let yesterday = parse_cli(["diddo", "yesterday", "--table"]).unwrap();
        let week = parse_cli(["diddo", "week", "--table"]).unwrap();
        let standup = parse_cli(["diddo", "standup", "--table"]).unwrap();

        assert!(format!("{default_today:?}").contains("table: true"));
        assert!(matches!(explicit_today.command, Some(Commands::Today(_))));
        assert!(format!("{explicit_today:?}").contains("table: true"));
        assert!(matches!(yesterday.command, Some(Commands::Yesterday(_))));
        assert!(format!("{yesterday:?}").contains("table: true"));
        assert!(matches!(week.command, Some(Commands::Week(_))));
        assert!(format!("{week:?}").contains("table: true"));
        assert!(matches!(standup.command, Some(Commands::Standup(_))));
        assert!(format!("{standup:?}").contains("table: true"));
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
    fn rejects_table_with_other_summary_output_flags() {
        let error = parse_cli(["diddo", "--table", "--json"]).unwrap_err();

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
                    table: false,
                    no_cache: false,
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
            table: false,
            no_cache: false,
        }));
        assert!(should_try_ai_summary(SummaryArgs {
            md: false,
            raw: false,
            json: true,
            table: false,
            no_cache: false,
        }));
        assert!(!should_try_ai_summary(SummaryArgs {
            md: false,
            raw: true,
            json: false,
            table: false,
            no_cache: false,
        }));
    }

    #[test]
    fn table_output_skips_ai_summary() {
        let (_, summary_args) =
            summary_request_from_cli(parse_cli(["diddo", "--table"]).unwrap()).unwrap();

        assert!(!should_try_ai_summary(summary_args));
        assert_eq!(format!("{:?}", output_format(summary_args)), "Table");
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
                table: false,
                no_cache: false,
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
            Some(
                "AI summary unavailable: no AI provider configured or detected. Falling back to raw output."
            )
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
        assert_eq!(
            output_format(SummaryArgs::default()),
            OutputFormat::Terminal
        );
        assert_eq!(
            output_format(SummaryArgs {
                md: true,
                raw: false,
                json: false,
                table: false,
                no_cache: false,
            }),
            OutputFormat::Markdown
        );
        assert_eq!(
            output_format(SummaryArgs {
                md: false,
                raw: false,
                json: true,
                table: false,
                no_cache: false,
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
            vec![last_commit.clone(), first_commit.clone(), second_commit],
        );

        assert_eq!(summary.total_commits, 3);
        assert_eq!(summary.project_count, 2);
        assert_eq!(
            summary.first_commit_time,
            format_commit_time(first_commit.committed_at)
        );
        assert_eq!(
            summary.last_commit_time,
            format_commit_time(last_commit.committed_at)
        );
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
                table: false,
                no_cache: false,
            },
        );
        let json = render_empty_summary(
            "2026-03-10 (today)",
            SummaryArgs {
                md: false,
                raw: false,
                json: true,
                table: false,
                no_cache: false,
            },
        );
        let table = render_empty_summary(
            "2026-03-10 (today)",
            summary_request_from_cli(parse_cli(["diddo", "--table"]).unwrap())
                .unwrap()
                .1,
        );

        assert_eq!(terminal, "No commits recorded for 2026-03-10 (today).");
        assert!(!terminal.contains("Profile:"));
        assert_eq!(
            markdown,
            "# 2026-03-10 (today)\n\nNo commits recorded for 2026-03-10 (today).\n"
        );
        assert_eq!(table, "No commits recorded for 2026-03-10 (today).");
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
    fn single_profile_unknown_when_all_commits_have_no_author_email() {
        let commits = vec![
            sample_commit("abc1", "repo-a", "/tmp/repo-a", 9, 15),
            sample_commit("def2", "repo-a", "/tmp/repo-a", 10, 30),
        ];
        let window = resolve_summary_window(
            SummaryPeriod::Today,
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
        );
        let database = crate::db::Database::open_in_memory().unwrap();
        let rendered = render_summary_output(
            &database,
            SummaryArgs {
                md: false,
                raw: false,
                json: false,
                table: false,
                no_cache: false,
            },
            window,
            commits,
            || Ok(AppConfig::default()),
            |_| {
                Ok(Box::new(SuccessProvider(
                    "Summary for unknown profile.".to_string(),
                )))
            },
        )
        .unwrap();

        assert!(rendered.output.contains("Profile: unknown"));
        assert!(rendered.output.contains("Summary for unknown profile."));
    }

    #[test]
    fn empty_summary_output_does_not_depend_on_config_loading() {
        let window = resolve_summary_window(
            SummaryPeriod::Today,
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
        );

        let database = crate::db::Database::open_in_memory().unwrap();
        let rendered = render_summary_output(
            &database,
            SummaryArgs {
                md: false,
                raw: false,
                json: true,
                table: false,
                no_cache: false,
            },
            window,
            Vec::new(),
            || Err(std::io::Error::other("bad config").into()),
            |_config| Ok(Box::new(SuccessProvider("unused".to_string()))),
        )
        .unwrap();

        assert_eq!(rendered.warning, None);
        assert!(
            rendered
                .output
                .contains("\"date_label\": \"2026-03-10 (today)\"")
        );
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
            author_email: None,
        }
    }

    fn sample_commit_at(
        hash: &str,
        repo_name: &str,
        repo_path: &str,
        committed_at: chrono::DateTime<Utc>,
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
            committed_at,
            author_email: None,
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

    /// Fails first summarize call, succeeds on the second.
    struct FailFirstProvider {
        call_count: Cell<usize>,
        success_message: String,
        failure_message: String,
    }

    impl FailFirstProvider {
        fn new(success_message: &str, failure_message: &str) -> Self {
            Self {
                call_count: Cell::new(0),
                success_message: success_message.to_string(),
                failure_message: failure_message.to_string(),
            }
        }
    }

    impl AiProvider for FailFirstProvider {
        fn summarize(
            &self,
            _commits: &[crate::db::Commit],
            _period: &str,
        ) -> crate::ai::Result<String> {
            let n = self.call_count.get();
            self.call_count.set(n + 1);
            if n == 0 {
                Err(AiError::new(self.failure_message.clone()))
            } else {
                Ok(self.success_message.clone())
            }
        }
    }

    fn commit_with_author(
        hash: &str,
        repo_name: &str,
        repo_path: &str,
        hour: u32,
        minute: u32,
        author_email: Option<&str>,
    ) -> crate::db::Commit {
        let mut c = sample_commit(hash, repo_name, repo_path, hour, minute);
        c.author_email = author_email.map(String::from);
        c
    }

    #[test]
    fn ai_failure_for_one_profile_shows_raw_for_that_profile_and_warning() {
        let commits = vec![
            commit_with_author("a1", "repo-a", "/tmp/repo-a", 9, 15, Some("alice@x.com")),
            commit_with_author("b1", "repo-b", "/tmp/repo-b", 10, 0, Some("bob@y.com")),
        ];
        let window = resolve_summary_window(
            SummaryPeriod::Today,
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
        );
        let database = crate::db::Database::open_in_memory().unwrap();
        let provider = FailFirstProvider::new("Bob summary.", "AI failed for first profile");
        let rendered = render_summary_output(
            &database,
            SummaryArgs {
                md: false,
                raw: false,
                json: false,
                table: false,
                no_cache: false,
            },
            window,
            commits,
            || Ok(AppConfig::default()),
            |_| Ok(Box::new(provider)),
        )
        .unwrap();

        assert!(rendered.output.contains("Profile: alice@x.com"));
        assert!(rendered.output.contains("Profile: bob@y.com"));
        assert!(rendered.output.contains("repo-a (1 commit)"));
        assert!(rendered.output.contains("a1  feat: update repo-a"));
        assert!(rendered.output.contains("Bob summary."));
        assert!(
            rendered
                .warning
                .as_deref()
                .unwrap()
                .contains("AI failed for first profile")
        );
    }

    #[test]
    fn metadata_shows_size_count_and_oldest_for_empty_database() {
        let database = crate::db::Database::open_in_memory().unwrap();

        let output = format_metadata(&database, 0).unwrap();

        assert!(output.contains("Database size:   0 bytes"));
        assert!(output.contains("Total commits:   0"));
        assert!(output.contains("Oldest commit:   -"));
    }

    #[test]
    fn metadata_shows_correct_stats_after_inserting_commits() {
        let database = crate::db::Database::open_in_memory().unwrap();

        let earlier = Utc.with_ymd_and_hms(2026, 3, 8, 10, 0, 0).unwrap();
        let later = Utc.with_ymd_and_hms(2026, 3, 10, 14, 0, 0).unwrap();

        database
            .insert_commit(&sample_commit_at(
                "aaa1111",
                "repo-a",
                "/tmp/repo-a",
                earlier,
            ))
            .unwrap();
        database
            .insert_commit(&sample_commit_at("bbb2222", "repo-b", "/tmp/repo-b", later))
            .unwrap();

        let output = format_metadata(&database, 50 * 1024 * 1024).unwrap();

        assert!(output.contains("Database size:   50.00 MB"));
        assert!(output.contains("Total commits:   2"));
        // The oldest commit line should contain a formatted date, not "-"
        assert!(!output.contains("Oldest commit:   -"));
        assert!(output.contains("Oldest commit:   2026-03-08"));
    }

    #[test]
    fn parses_standup_subcommand_with_summary_flags() {
        let standup = parse_cli(["diddo", "standup"]).unwrap();
        let standup_md = parse_cli(["diddo", "standup", "--md"]).unwrap();

        assert_eq!(
            standup.command,
            Some(Commands::Standup(SummaryArgs::default()))
        );
        assert_eq!(
            standup_md.command,
            Some(Commands::Standup(SummaryArgs {
                md: true,
                raw: false,
                json: false,
                table: false,
                no_cache: false,
            }))
        );
    }

    #[test]
    fn derives_standup_summary_request() {
        let standup = summary_request_from_cli(parse_cli(["diddo", "standup"]).unwrap());

        assert_eq!(
            standup,
            Some((SummaryPeriod::Standup, SummaryArgs::default()))
        );
    }

    #[test]
    fn resolves_standup_window_with_exact_24h_bounds() {
        let today = NaiveDate::from_ymd_opt(2026, 3, 12).unwrap();

        let window = resolve_summary_window(SummaryPeriod::Standup, today);

        assert_eq!(window.date_label, "last 24 hours (standup)");
        assert_eq!(window.ai_period, "the last 24 hours");
        assert!(window.exact_bounds.is_some());
        let (from, to) = window.exact_bounds.unwrap();
        let diff = to - from;
        assert_eq!(diff.num_hours(), 24);
    }

    #[test]
    fn standup_renders_ai_summary_for_commits() {
        let commits = vec![
            sample_commit("abc1", "repo-a", "/tmp/repo-a", 9, 15),
            sample_commit("def2", "repo-b", "/tmp/repo-b", 10, 30),
        ];
        let window = resolve_summary_window(
            SummaryPeriod::Standup,
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
        );
        let database = crate::db::Database::open_in_memory().unwrap();
        let rendered = render_summary_output(
            &database,
            SummaryArgs::default(),
            window,
            commits,
            || Ok(AppConfig::default()),
            |_| {
                Ok(Box::new(SuccessProvider(
                    "Standup summary for last 24h.".to_string(),
                )))
            },
        )
        .unwrap();

        assert!(rendered.output.contains("last 24 hours (standup)"));
        assert!(rendered.output.contains("Standup summary for last 24h."));
    }

    #[test]
    fn standup_renders_empty_period_message_when_no_commits() {
        let window = resolve_summary_window(
            SummaryPeriod::Standup,
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
        );
        let database = crate::db::Database::open_in_memory().unwrap();
        let rendered = render_summary_output(
            &database,
            SummaryArgs::default(),
            window,
            Vec::new(),
            || Err(std::io::Error::other("should not load config").into()),
            |_| Ok(Box::new(SuccessProvider("unused".to_string()))),
        )
        .unwrap();

        assert!(
            rendered
                .output
                .contains("No commits recorded for last 24 hours (standup)")
        );
        assert_eq!(rendered.warning, None);
    }

    #[test]
    fn standup_raw_skips_ai_and_shows_grouped_commits() {
        let commits = vec![sample_commit("abc1", "repo-a", "/tmp/repo-a", 9, 15)];
        let window = resolve_summary_window(
            SummaryPeriod::Standup,
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
        );
        let database = crate::db::Database::open_in_memory().unwrap();
        let rendered = render_summary_output(
            &database,
            SummaryArgs {
                md: false,
                raw: true,
                json: false,
                table: false,
                no_cache: false,
            },
            window,
            commits,
            || Ok(AppConfig::default()),
            |_| Err(crate::ai::AiError::new("should not be called")),
        )
        .unwrap();

        assert!(rendered.output.contains("last 24 hours (standup)"));
        assert!(rendered.output.contains("repo-a"));
        assert_eq!(rendered.warning, None);
    }

    #[test]
    fn standup_json_output_includes_date_label() {
        let commits = vec![sample_commit("abc1", "repo-a", "/tmp/repo-a", 9, 15)];
        let window = resolve_summary_window(
            SummaryPeriod::Standup,
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
        );
        let database = crate::db::Database::open_in_memory().unwrap();
        let rendered = render_summary_output(
            &database,
            SummaryArgs {
                md: false,
                raw: false,
                json: true,
                table: false,
                no_cache: false,
            },
            window,
            commits,
            || Ok(AppConfig::default()),
            |_| Ok(Box::new(SuccessProvider("JSON standup.".to_string()))),
        )
        .unwrap();

        assert!(
            rendered
                .output
                .contains("\"date_label\": \"last 24 hours (standup)\"")
        );
    }

    #[test]
    fn table_output_renders_repo_totals_without_ai() {
        let commits = vec![
            sample_commit("abc1", "repo-a", "/tmp/repo-a", 9, 15),
            sample_commit("def2", "repo-a", "/tmp/repo-a", 10, 30),
            sample_commit("ghi3", "repo-b", "/tmp/repo-b", 11, 0),
        ];
        let window = resolve_summary_window(
            SummaryPeriod::Today,
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
        );
        let (_, summary_args) =
            summary_request_from_cli(parse_cli(["diddo", "--table"]).unwrap()).unwrap();
        let database = crate::db::Database::open_in_memory().unwrap();
        let rendered = render_summary_output(
            &database,
            summary_args,
            window,
            commits,
            || Ok(AppConfig::default()),
            |_| Err(crate::ai::AiError::new("should not be called")),
        )
        .unwrap();

        assert!(rendered.output.contains("2026-03-10 (today)"));
        assert!(rendered.output.contains("repo-a"));
        assert!(rendered.output.contains("repo-b"));
        assert!(rendered.output.contains("Total"));
        assert!(rendered.output.contains("100.0%"));
        assert_eq!(rendered.warning, None);
    }

    #[test]
    fn default_terminal_output_appends_repo_table_after_ai_summary() {
        let commits = vec![
            sample_commit("abc1", "repo-a", "/tmp/repo-a", 9, 15),
            sample_commit("def2", "repo-a", "/tmp/repo-a", 10, 30),
            sample_commit("ghi3", "repo-b", "/tmp/repo-b", 11, 0),
        ];
        let window = resolve_summary_window(
            SummaryPeriod::Today,
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
        );
        let database = crate::db::Database::open_in_memory().unwrap();
        let rendered = render_summary_output(
            &database,
            SummaryArgs::default(),
            window,
            commits,
            || Ok(AppConfig::default()),
            |_| Ok(Box::new(SuccessProvider("AI summary text.".to_string()))),
        )
        .unwrap();

        let ai_pos = rendered.output.find("AI summary text.").unwrap();
        let table_pos = rendered.output.find("repository").unwrap();
        let footer_pos = rendered.output.find("3 commits").unwrap();
        assert!(ai_pos < table_pos, "AI summary should appear before table");
        assert!(table_pos < footer_pos, "table should appear before footer");
        assert!(rendered.output.contains("Total"));
        assert!(rendered.output.contains("100.0%"));
    }

    #[test]
    fn markdown_output_appends_repo_table_by_default() {
        let commits = vec![
            sample_commit("abc1", "repo-a", "/tmp/repo-a", 9, 15),
            sample_commit("def2", "repo-b", "/tmp/repo-b", 10, 30),
        ];
        let window = resolve_summary_window(
            SummaryPeriod::Today,
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
        );
        let database = crate::db::Database::open_in_memory().unwrap();
        let rendered = render_summary_output(
            &database,
            SummaryArgs {
                md: true,
                raw: false,
                json: false,
                table: false,
                no_cache: false,
            },
            window,
            commits,
            || Ok(AppConfig::default()),
            |_| Ok(Box::new(SuccessProvider("MD summary.".to_string()))),
        )
        .unwrap();

        assert!(rendered.output.contains("| repository |"));
        assert!(rendered.output.contains("| **Total** |"));
        let summary_pos = rendered.output.find("MD summary.").unwrap();
        let table_pos = rendered.output.find("| repository |").unwrap();
        assert!(summary_pos < table_pos);
    }

    #[test]
    fn raw_output_does_not_append_repo_table() {
        let commits = vec![
            sample_commit("abc1", "repo-a", "/tmp/repo-a", 9, 15),
            sample_commit("def2", "repo-b", "/tmp/repo-b", 10, 30),
        ];
        let window = resolve_summary_window(
            SummaryPeriod::Today,
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
        );
        let database = crate::db::Database::open_in_memory().unwrap();
        let rendered = render_summary_output(
            &database,
            SummaryArgs {
                md: false,
                raw: true,
                json: false,
                table: false,
                no_cache: false,
            },
            window,
            commits,
            || Ok(AppConfig::default()),
            |_| Err(crate::ai::AiError::new("should not be called")),
        )
        .unwrap();

        assert!(rendered.output.contains("repo-a"));
        assert!(!rendered.output.contains("repository"));
        assert!(!rendered.output.contains("percentage"));
        assert!(!rendered.output.contains("| repository |"));
    }

    #[test]
    fn json_output_remains_unchanged_without_table_section() {
        let commits = vec![
            sample_commit("abc1", "repo-a", "/tmp/repo-a", 9, 15),
            sample_commit("def2", "repo-b", "/tmp/repo-b", 10, 30),
        ];
        let window = resolve_summary_window(
            SummaryPeriod::Today,
            NaiveDate::from_ymd_opt(2026, 3, 10).unwrap(),
        );
        let database = crate::db::Database::open_in_memory().unwrap();
        let rendered = render_summary_output(
            &database,
            SummaryArgs {
                md: false,
                raw: false,
                json: true,
                table: false,
                no_cache: false,
            },
            window,
            commits,
            || Ok(AppConfig::default()),
            |_| Ok(Box::new(SuccessProvider("JSON summary.".to_string()))),
        )
        .unwrap();

        assert!(rendered.output.contains("\"date_label\""));
        assert!(!rendered.output.contains("repository"));
        assert!(!rendered.output.contains("percentage"));
        assert!(!rendered.output.contains("| repository |"));
    }

    #[test]
    fn date_based_windows_have_no_exact_bounds() {
        let today = NaiveDate::from_ymd_opt(2026, 3, 12).unwrap();

        assert!(
            resolve_summary_window(SummaryPeriod::Today, today)
                .exact_bounds
                .is_none()
        );
        assert!(
            resolve_summary_window(SummaryPeriod::Yesterday, today)
                .exact_bounds
                .is_none()
        );
        assert!(
            resolve_summary_window(SummaryPeriod::Week, today)
                .exact_bounds
                .is_none()
        );
    }

    #[test]
    fn formats_file_size_in_bytes_kb_mb_gb() {
        assert_eq!(format_file_size(0), "0 bytes");
        assert_eq!(format_file_size(1), "0.00 KB");
        assert_eq!(format_file_size(512 * 1024), "512.00 KB");
        assert_eq!(format_file_size(1024 * 1024), "1.00 MB");
        assert_eq!(format_file_size(50 * 1024 * 1024), "50.00 MB");
        assert_eq!(format_file_size(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_file_size(1536 * 1024 * 1024), "1.50 GB");
    }
}
