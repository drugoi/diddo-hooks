pub mod api_provider;
pub mod cli_provider;

use std::{error::Error, fmt, io};

use api_provider::{ApiKind, ApiProvider};
use cli_provider::{CliProvider, CliTool};

use crate::{config::AiConfig, db::Commit};

#[allow(dead_code)]
pub trait AiProvider {
    fn summarize(&self, commits: &[Commit], period: &str) -> Result<String>;
}

pub type Result<T> = std::result::Result<T, AiError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiError {
    message: String,
}

impl AiError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for AiError {}

impl From<io::Error> for AiError {
    fn from(error: io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<reqwest::Error> for AiError {
    fn from(error: reqwest::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<serde_json::Error> for AiError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderChoice {
    Cli(CliTool),
    Api(ApiKind),
}

#[allow(dead_code)]
pub fn create_provider(config: &AiConfig) -> Result<Box<dyn AiProvider>> {
    let providers = select_provider(config, &cli_provider::detect_installed_tools())?
        .into_iter()
        .map(|choice| match choice {
            ProviderChoice::Cli(tool) => Ok(Box::new(CliProvider::new(
                tool,
                config.resolved_prompt_instructions().map(String::from),
            )) as Box<dyn AiProvider>),
            ProviderChoice::Api(kind) => {
                Ok(Box::new(ApiProvider::from_config(config, kind)?) as Box<dyn AiProvider>)
            }
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Box::new(FallbackProvider::new(providers)))
}

fn select_provider(
    config: &AiConfig,
    available_cli_tools: &[CliTool],
) -> Result<Vec<ProviderChoice>> {
    let cli_preference = config.normalized_cli_preference();

    if let Some(preference) = cli_preference.as_deref() {
        match preference {
            "api" => return Ok(vec![select_api_provider(config, true)?]),
            "cli" => {}
            _ => {
                let Some(tool) = CliTool::from_name(preference) else {
                    return Err(AiError::new(format!(
                        "invalid ai.cli.prefer value: {preference} (expected one of: api, cli, claude, codex, opencode, cursor-agent)"
                    )));
                };

                if !available_cli_tools.contains(&tool) {
                    if let Some(api_choice) = try_select_api_provider(config, false)? {
                        return Ok(vec![api_choice]);
                    }

                    return Err(AiError::new(format!(
                        "preferred CLI tool {} is not installed",
                        tool.display_name()
                    )));
                }

                let mut choices = vec![ProviderChoice::Cli(tool)];
                if let Some(api_choice) = try_select_api_provider(config, false)? {
                    choices.push(api_choice);
                }

                return Ok(choices);
            }
        }
    }

    let mut choices = available_cli_tools
        .iter()
        .copied()
        .map(ProviderChoice::Cli)
        .collect::<Vec<_>>();

    if let Some(api_choice) = try_select_api_provider(config, choices.is_empty())? {
        choices.push(api_choice);
    }

    if choices.is_empty() {
        return Err(AiError::new("no AI provider configured or detected"));
    }

    Ok(choices)
}

fn select_api_provider(config: &AiConfig, required: bool) -> Result<ProviderChoice> {
    try_select_api_provider(config, required)?
        .ok_or_else(|| AiError::new("no AI provider configured or detected"))
}

fn try_select_api_provider(config: &AiConfig, required: bool) -> Result<Option<ProviderChoice>> {
    let Some(provider_name) = config.resolved_provider() else {
        return if required {
            Err(AiError::new("no AI provider configured or detected"))
        } else {
            Ok(None)
        };
    };

    let Some(api_kind) = ApiKind::from_name(&provider_name) else {
        return if required {
            Err(AiError::new(format!(
                "unsupported AI provider: {provider_name}"
            )))
        } else {
            Ok(None)
        };
    };

    if config.resolved_api_key().is_none() {
        return if required {
            Err(AiError::new(format!("missing API key for {provider_name}")))
        } else {
            Ok(None)
        };
    }

    Ok(Some(ProviderChoice::Api(api_kind)))
}

#[allow(dead_code)]
pub fn primary_provider_identity(config: &AiConfig) -> Result<(String, String)> {
    let choices = select_provider(config, &cli_provider::detect_installed_tools())?;
    let first = choices
        .first()
        .ok_or_else(|| AiError::new("no AI provider configured or detected"))?;
    Ok(match first {
        ProviderChoice::Cli(tool) => (
            tool.display_name().to_ascii_lowercase(),
            "default".to_string(),
        ),
        ProviderChoice::Api(kind) => {
            let id = config
                .resolved_provider()
                .unwrap_or_else(|| kind.display_name().to_ascii_lowercase());
            let model = config
                .resolved_model()
                .unwrap_or_else(|| kind.default_model().to_string());
            (id, model)
        }
    })
}

#[allow(dead_code)]
struct FallbackProvider {
    providers: Vec<Box<dyn AiProvider>>,
}

#[allow(dead_code)]
impl FallbackProvider {
    fn new(providers: Vec<Box<dyn AiProvider>>) -> Self {
        Self { providers }
    }
}

impl AiProvider for FallbackProvider {
    fn summarize(&self, commits: &[Commit], period: &str) -> Result<String> {
        let mut errors = Vec::new();

        for provider in &self.providers {
            match provider.summarize(commits, period) {
                Ok(summary) => return Ok(summary),
                Err(error) => errors.push(error.to_string()),
            }
        }

        Err(AiError::new(format!(
            "all AI providers failed: {}",
            errors.join(" | ")
        )))
    }
}

#[allow(dead_code)]
pub fn build_prompt(
    commits: &[Commit],
    period: &str,
    instructions_override: Option<&str>,
) -> String {
    if let Some(s) = instructions_override {
        let mut prompt = s.to_string();
        prompt.push_str(&format!(
            "\n\nPeriod: {period}\nCommit count: {}\n\nCommits:\n",
            commits.len()
        ));
        if commits.is_empty() {
            prompt.push_str("- No recorded commits.\n");
        } else {
            for (index, commit) in commits.iter().enumerate() {
                prompt.push_str(&format!(
                    "{}. [{}] {} ({}) on {} at {}; files: {}, +{}, -{}\n",
                    index + 1,
                    commit.repo_name,
                    commit.message,
                    commit.hash,
                    commit.branch,
                    commit.committed_at.to_rfc3339(),
                    commit.files_changed,
                    commit.insertions,
                    commit.deletions
                ));
            }
        }
        return prompt;
    }

    let mut prompt = format!(
        "You are summarizing git activity for {period}.\n\
         Write a concise status update with the main themes, notable repos, and momentum.\n\
         Use only the commit data below.\n\n\
         Period: {period}\n\
         Commit count: {}\n\n\
         Commits:\n",
        commits.len()
    );

    if commits.is_empty() {
        prompt.push_str("- No recorded commits.\n");
    } else {
        for (index, commit) in commits.iter().enumerate() {
            prompt.push_str(&format!(
                "{}. [{}] {} ({}) on {} at {}; files: {}, +{}, -{}\n",
                index + 1,
                commit.repo_name,
                commit.message,
                commit.hash,
                commit.branch,
                commit.committed_at.to_rfc3339(),
                commit.files_changed,
                commit.insertions,
                commit.deletions
            ));
        }
    }

    prompt
        .push_str("\nReturn plain text only. Keep it brief and useful, in 2 short paragraphs max.");

    prompt
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{
        build_prompt, select_provider, AiError, AiProvider, FallbackProvider, ProviderChoice,
    };
    use crate::{
        ai::{api_provider::ApiKind, cli_provider::CliTool},
        config::{AiCliConfig, AiConfig},
        db::Commit,
    };

    #[test]
    fn prefers_configured_cli_tool_when_it_is_available() {
        let config = AiConfig {
            cli: AiCliConfig {
                prefer: Some("codex".to_string()),
            },
            ..AiConfig::default()
        };

        let choices = select_provider(&config, &[CliTool::Claude, CliTool::Codex]).unwrap();

        assert_eq!(choices, vec![ProviderChoice::Cli(CliTool::Codex)]);
    }

    #[test]
    fn falls_back_to_detected_cli_tool_before_api() {
        let config = AiConfig {
            provider: Some("openai".to_string()),
            api_key: Some("openai-key".to_string()),
            cli: AiCliConfig {
                prefer: Some("cli".to_string()),
            },
            ..AiConfig::default()
        };

        let choices = select_provider(&config, &[CliTool::Claude]).unwrap();

        assert_eq!(
            choices,
            vec![
                ProviderChoice::Cli(CliTool::Claude),
                ProviderChoice::Api(ApiKind::OpenAi),
            ]
        );
    }

    #[test]
    fn uses_api_when_config_explicitly_prefers_it() {
        let config = AiConfig {
            provider: Some("anthropic".to_string()),
            api_key: Some("anthropic-key".to_string()),
            cli: AiCliConfig {
                prefer: Some("api".to_string()),
            },
            ..AiConfig::default()
        };

        let choices = select_provider(&config, &[CliTool::Claude, CliTool::Codex]).unwrap();

        assert_eq!(choices, vec![ProviderChoice::Api(ApiKind::Anthropic)]);
    }

    #[test]
    fn errors_when_specific_preferred_cli_is_unavailable() {
        let config = AiConfig {
            provider: Some("openai".to_string()),
            api_key: Some("openai-key".to_string()),
            cli: AiCliConfig {
                prefer: Some("codex".to_string()),
            },
            ..AiConfig::default()
        };

        let choices = select_provider(&config, &[CliTool::Claude]).unwrap();

        assert_eq!(choices, vec![ProviderChoice::Api(ApiKind::OpenAi)]);
    }

    #[test]
    fn errors_when_specific_preferred_cli_is_unavailable_and_no_api_fallback_exists() {
        let config = AiConfig {
            cli: AiCliConfig {
                prefer: Some("codex".to_string()),
            },
            ..AiConfig::default()
        };

        let error = select_provider(&config, &[CliTool::Claude]).unwrap_err();

        assert_eq!(
            error.to_string(),
            "preferred CLI tool codex is not installed"
        );
    }

    #[test]
    fn errors_when_cli_preference_value_is_invalid() {
        let config = AiConfig {
            cli: AiCliConfig {
                prefer: Some("claud".to_string()),
            },
            ..AiConfig::default()
        };

        let error = select_provider(&config, &[CliTool::Claude]).unwrap_err();

        assert_eq!(
            error.to_string(),
            "invalid ai.cli.prefer value: claud (expected one of: api, cli, claude, codex, opencode, cursor-agent)"
        );
    }

    #[test]
    fn ignores_invalid_optional_api_provider_when_detected_cli_is_available() {
        let config = AiConfig {
            provider: Some("openia".to_string()),
            api_key: Some("bad-key".to_string()),
            ..AiConfig::default()
        };

        let choices = select_provider(&config, &[CliTool::Claude]).unwrap();

        assert_eq!(choices, vec![ProviderChoice::Cli(CliTool::Claude)]);
    }

    #[test]
    fn ignores_invalid_optional_api_provider_when_preferred_cli_is_available() {
        let config = AiConfig {
            provider: Some("openia".to_string()),
            api_key: Some("bad-key".to_string()),
            cli: AiCliConfig {
                prefer: Some("codex".to_string()),
            },
            ..AiConfig::default()
        };

        let choices = select_provider(&config, &[CliTool::Codex]).unwrap();

        assert_eq!(choices, vec![ProviderChoice::Cli(CliTool::Codex)]);
    }

    #[test]
    fn errors_when_invalid_api_provider_is_the_only_available_path() {
        let config = AiConfig {
            provider: Some("openia".to_string()),
            api_key: Some("bad-key".to_string()),
            ..AiConfig::default()
        };

        let error = select_provider(&config, &[]).unwrap_err();

        assert_eq!(error.to_string(), "unsupported AI provider: openia");
    }

    #[test]
    fn errors_when_no_cli_or_api_provider_is_available() {
        let error = select_provider(&AiConfig::default(), &[]).unwrap_err();

        assert_eq!(error.to_string(), "no AI provider configured or detected");
    }

    #[test]
    fn prompt_includes_period_and_commit_details() {
        let prompt = build_prompt(&[sample_commit()], "today", None);

        assert!(prompt.contains("Period: today"));
        assert!(prompt.contains("[diddo] feat: add AI summaries"));
        assert!(prompt.contains("files: 4, +28, -6"));
    }

    #[test]
    fn build_prompt_with_custom_instructions_uses_override_and_structured_block() {
        let commits = vec![sample_commit()];
        let prompt = build_prompt(&commits, "today", Some("Custom instructions here."));

        assert!(prompt.starts_with("Custom instructions here."));
        assert!(prompt.contains("Period: today"));
        assert!(prompt.contains("Commit count: 1"));
        assert!(prompt.contains("[diddo] feat: add AI summaries"));
        assert!(!prompt.contains("Return plain text only"));
    }

    #[test]
    fn build_prompt_with_none_uses_default_instructions() {
        let commits = vec![sample_commit()];
        let prompt = build_prompt(&commits, "week", None);

        assert!(prompt.contains("You are summarizing git activity for week."));
        assert!(prompt.contains("Return plain text only. Keep it brief"));
        assert!(prompt.contains("Period: week"));
    }

    #[test]
    fn falls_back_to_next_provider_when_first_runtime_attempt_fails() {
        let provider = FallbackProvider::new(vec![
            Box::new(FailingProvider("claude failed".to_string())) as Box<dyn AiProvider>,
            Box::new(SuccessProvider("API summary".to_string())) as Box<dyn AiProvider>,
        ]);

        let summary = provider.summarize(&[sample_commit()], "today").unwrap();

        assert_eq!(summary, "API summary");
    }

    struct FailingProvider(String);

    impl AiProvider for FailingProvider {
        fn summarize(&self, _commits: &[Commit], _period: &str) -> super::Result<String> {
            Err(AiError::new(self.0.clone()))
        }
    }

    struct SuccessProvider(String);

    impl AiProvider for SuccessProvider {
        fn summarize(&self, _commits: &[Commit], _period: &str) -> super::Result<String> {
            Ok(self.0.clone())
        }
    }

    fn sample_commit() -> Commit {
        Commit {
            id: None,
            hash: "abc1234".to_string(),
            message: "feat: add AI summaries".to_string(),
            repo_path: "/Users/example/projects/diddo".to_string(),
            repo_name: "diddo".to_string(),
            branch: "feature/diddo".to_string(),
            files_changed: 4,
            insertions: 28,
            deletions: 6,
            committed_at: Utc.with_ymd_and_hms(2026, 3, 10, 9, 15, 0).unwrap(),
            author_email: None,
        }
    }
}
