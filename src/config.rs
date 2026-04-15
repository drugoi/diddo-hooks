use std::{fs, io, path::Path};

use serde::Deserialize;

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub ai: AiConfig,
    pub update: UpdateConfig,
    pub filters: FiltersConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    pub auto_check: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self { auto_check: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct FiltersConfig {
    pub ignored_profiles: Vec<String>,
}

impl Default for FiltersConfig {
    fn default() -> Self {
        Self {
            ignored_profiles: vec![String::from("test@test.com")],
        }
    }
}

impl FiltersConfig {
    pub fn is_ignored(&self, email: Option<&str>) -> bool {
        let Some(candidate) = email.map(str::trim).filter(|s| !s.is_empty()) else {
            return false;
        };
        self.ignored_profiles
            .iter()
            .map(|entry| entry.trim())
            .filter(|entry| !entry.is_empty())
            .any(|entry| entry.eq_ignore_ascii_case(candidate))
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    pub provider: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub prompt_instructions: Option<String>,
    pub cli: AiCliConfig,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AiCliConfig {
    pub prefer: Option<String>,
}

impl AppConfig {
    pub fn load(path: &Path) -> io::Result<Self> {
        let mut config = if path.exists() {
            let contents = fs::read_to_string(path)?;
            toml::from_str::<Self>(&contents).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("failed to parse config file {}: {error}", path.display()),
                )
            })?
        } else {
            Self::default()
        };

        config.ai.apply_environment_defaults();
        Ok(config)
    }
}

#[allow(dead_code)]
impl AiConfig {
    pub fn normalized_provider(&self) -> Option<String> {
        normalize_provider(self.provider.as_deref())
    }

    pub fn normalized_cli_preference(&self) -> Option<String> {
        normalize_value(self.cli.prefer.as_deref()).map(|value| value.to_ascii_lowercase())
    }

    pub fn resolved_provider(&self) -> Option<String> {
        self.normalized_provider()
            .or_else(infer_provider_from_environment)
    }

    pub fn resolved_model(&self) -> Option<String> {
        normalize_value(self.model.as_deref())
    }

    pub fn resolved_api_key(&self) -> Option<String> {
        normalize_value(self.api_key.as_deref())
    }

    pub fn apply_environment_defaults(&mut self) {
        if self.normalized_provider().is_none()
            && let Some(inferred) = infer_provider_from_environment()
        {
            self.provider = Some(inferred);
        }

        if normalize_value(self.api_key.as_deref()).is_none() {
            let env_key = match self.normalized_provider().as_deref() {
                Some("openai") => read_env("DIDDO_OPENAI_KEY"),
                Some("anthropic") => read_env("DIDDO_ANTHROPIC_KEY"),
                _ => None,
            };
            if let Some(key) = env_key {
                self.api_key = Some(key);
            }
        }
    }

    pub fn resolved_prompt_instructions(&self) -> Option<&str> {
        self.prompt_instructions
            .as_deref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
    }
}

#[allow(dead_code)]
fn normalize_provider(provider: Option<&str>) -> Option<String> {
    provider
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .map(str::to_ascii_lowercase)
}

#[allow(dead_code)]
fn infer_provider_from_environment() -> Option<String> {
    match (
        read_env("DIDDO_OPENAI_KEY").is_some(),
        read_env("DIDDO_ANTHROPIC_KEY").is_some(),
    ) {
        (true, false) => Some(String::from("openai")),
        (false, true) => Some(String::from("anthropic")),
        _ => None,
    }
}

#[allow(dead_code)]
fn normalize_value(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[allow(dead_code)]
fn read_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::{Mutex, OnceLock},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::AppConfig;

    #[test]
    fn returns_default_config_when_file_does_not_exist() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        unsafe {
            std::env::remove_var("DIDDO_OPENAI_KEY");
            std::env::remove_var("DIDDO_ANTHROPIC_KEY");
        }

        let temp = temp_dir("missing-config");
        let missing = temp.join("config.toml");

        let config = AppConfig::load(&missing).unwrap();

        assert_eq!(config, AppConfig::default());

        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn parses_ai_settings_from_toml_file() {
        let temp = temp_dir("parse-config");
        let config_path = temp.join("config.toml");

        fs::write(
            &config_path,
            r#"[ai]
provider = "anthropic"
model = "claude-sonnet-4-6"

[ai.cli]
prefer = "cli"
"#,
        )
        .unwrap();

        let config = AppConfig::load(&config_path).unwrap();

        assert_eq!(config.ai.provider.as_deref(), Some("anthropic"));
        assert_eq!(config.ai.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(config.ai.cli.prefer.as_deref(), Some("cli"));

        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn falls_back_to_provider_api_key_from_environment() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = temp_dir("env-fallback");
        let config_path = temp.join("config.toml");

        fs::write(
            &config_path,
            r#"[ai]
provider = "openai"
"#,
        )
        .unwrap();

        unsafe {
            std::env::set_var("DIDDO_OPENAI_KEY", "openai-from-env");
            std::env::remove_var("DIDDO_ANTHROPIC_KEY");
        }

        let config = AppConfig::load(&config_path).unwrap();

        assert_eq!(config.ai.resolved_provider().as_deref(), Some("openai"));
        assert_eq!(
            config.ai.resolved_api_key().as_deref(),
            Some("openai-from-env")
        );

        unsafe {
            std::env::remove_var("DIDDO_OPENAI_KEY");
            std::env::remove_var("DIDDO_ANTHROPIC_KEY");
        }
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn prefers_file_api_key_over_environment_fallback() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = temp_dir("file-key-wins");
        let config_path = temp.join("config.toml");

        fs::write(
            &config_path,
            r#"[ai]
provider = "anthropic"
api_key = "file-key"
"#,
        )
        .unwrap();

        unsafe {
            std::env::set_var("DIDDO_ANTHROPIC_KEY", "anthropic-from-env");
        }

        let config = AppConfig::load(&config_path).unwrap();

        assert_eq!(config.ai.resolved_api_key().as_deref(), Some("file-key"));

        unsafe {
            std::env::remove_var("DIDDO_ANTHROPIC_KEY");
        }
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn missing_file_can_still_resolve_provider_and_api_key_from_environment() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = temp_dir("env-only-missing-file");
        let missing = temp.join("config.toml");

        unsafe {
            std::env::set_var("DIDDO_OPENAI_KEY", "openai-from-env");
            std::env::remove_var("DIDDO_ANTHROPIC_KEY");
        }

        let config = AppConfig::load(&missing).unwrap();

        assert_eq!(config.ai.provider.as_deref(), Some("openai"));
        assert_eq!(config.ai.resolved_provider().as_deref(), Some("openai"));
        assert_eq!(
            config.ai.resolved_api_key().as_deref(),
            Some("openai-from-env")
        );

        unsafe {
            std::env::remove_var("DIDDO_OPENAI_KEY");
            std::env::remove_var("DIDDO_ANTHROPIC_KEY");
        }
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn provider_less_config_can_resolve_sensibly_from_environment() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = temp_dir("provider-less-config");
        let config_path = temp.join("config.toml");

        fs::write(
            &config_path,
            r#"[ai]
model = "claude-sonnet-4-6"

[ai.cli]
prefer = "api"
"#,
        )
        .unwrap();

        unsafe {
            std::env::remove_var("DIDDO_OPENAI_KEY");
            std::env::set_var("DIDDO_ANTHROPIC_KEY", "anthropic-from-env");
        }

        let config = AppConfig::load(&config_path).unwrap();

        assert_eq!(config.ai.provider.as_deref(), Some("anthropic"));
        assert_eq!(config.ai.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(config.ai.resolved_provider().as_deref(), Some("anthropic"));
        assert_eq!(
            config.ai.resolved_api_key().as_deref(),
            Some("anthropic-from-env")
        );

        unsafe {
            std::env::remove_var("DIDDO_OPENAI_KEY");
            std::env::remove_var("DIDDO_ANTHROPIC_KEY");
        }
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn normalizes_mixed_case_and_whitespace_provider_values() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = temp_dir("normalized-provider");
        let config_path = temp.join("config.toml");

        fs::write(
            &config_path,
            r#"[ai]
provider = "  AnThRoPiC  "
"#,
        )
        .unwrap();

        unsafe {
            std::env::remove_var("DIDDO_OPENAI_KEY");
            std::env::set_var("DIDDO_ANTHROPIC_KEY", "anthropic-from-env");
        }

        let config = AppConfig::load(&config_path).unwrap();

        assert_eq!(config.ai.provider.as_deref(), Some("  AnThRoPiC  "));
        assert_eq!(
            config.ai.normalized_provider().as_deref(),
            Some("anthropic")
        );
        assert_eq!(config.ai.resolved_provider().as_deref(), Some("anthropic"));
        assert_eq!(
            config.ai.resolved_api_key().as_deref(),
            Some("anthropic-from-env")
        );

        unsafe {
            std::env::remove_var("DIDDO_OPENAI_KEY");
            std::env::remove_var("DIDDO_ANTHROPIC_KEY");
        }
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn normalizes_cli_preference_and_model_values() {
        let temp = temp_dir("normalized-cli-preference");
        let config_path = temp.join("config.toml");

        fs::write(
            &config_path,
            r#"[ai]
model = "  gpt-4o-mini  "

[ai.cli]
prefer = "  CoDeX  "
"#,
        )
        .unwrap();

        let config = AppConfig::load(&config_path).unwrap();

        assert_eq!(
            config.ai.normalized_cli_preference().as_deref(),
            Some("codex")
        );
        assert_eq!(config.ai.resolved_model().as_deref(), Some("gpt-4o-mini"));

        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn parses_prompt_instructions_from_toml() {
        let temp = temp_dir("prompt-instructions-parse");
        let config_path = temp.join("config.toml");

        fs::write(
            &config_path,
            r#"[ai]
prompt_instructions = " Summarize in German. One paragraph. "
"#,
        )
        .unwrap();

        let config = AppConfig::load(&config_path).unwrap();

        assert_eq!(
            config.ai.prompt_instructions.as_deref(),
            Some(" Summarize in German. One paragraph. ")
        );
        assert_eq!(
            config.ai.resolved_prompt_instructions(),
            Some("Summarize in German. One paragraph.")
        );

        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn prompt_instructions_empty_or_missing_returns_none() {
        let temp = temp_dir("prompt-instructions-empty");
        let missing = temp.join("config.toml");

        let config = AppConfig::load(&missing).unwrap();
        assert_eq!(config.ai.resolved_prompt_instructions(), None);

        let with_empty = temp.join("with_empty.toml");
        fs::write(&with_empty, "[ai]\nprompt_instructions = \"\"\n").unwrap();
        let config = AppConfig::load(&with_empty).unwrap();
        assert_eq!(config.ai.resolved_prompt_instructions(), None);

        let with_whitespace = temp.join("with_ws.toml");
        fs::write(
            &with_whitespace,
            r#"[ai]
prompt_instructions = "  \n\t "
"#,
        )
        .unwrap();
        let config = AppConfig::load(&with_whitespace).unwrap();
        assert_eq!(config.ai.resolved_prompt_instructions(), None);

        fs::remove_dir_all(temp).unwrap();
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let unique = format!(
            "{prefix}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn update_auto_check_defaults_to_true() {
        let temp = temp_dir("update-default");
        let missing = temp.join("config.toml");

        let config = AppConfig::load(&missing).unwrap();

        assert!(config.update.auto_check);

        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn update_auto_check_can_be_disabled() {
        let temp = temp_dir("update-disabled");
        let config_path = temp.join("config.toml");

        fs::write(&config_path, "[update]\nauto_check = false\n").unwrap();

        let config = AppConfig::load(&config_path).unwrap();

        assert!(!config.update.auto_check);

        fs::remove_dir_all(temp).unwrap();
    }

    fn env_lock() -> &'static Mutex<()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn filters_default_contains_test_profile() {
        let temp = temp_dir("filters-default");
        let missing = temp.join("config.toml");

        let config = AppConfig::load(&missing).unwrap();

        assert_eq!(config.filters.ignored_profiles, vec!["test@test.com"]);
        assert!(config.filters.is_ignored(Some("test@test.com")));

        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn filters_user_list_replaces_defaults() {
        let temp = temp_dir("filters-override");
        let config_path = temp.join("config.toml");

        fs::write(
            &config_path,
            r#"[filters]
ignored_profiles = ["bot@example.com", "ci@example.com"]
"#,
        )
        .unwrap();

        let config = AppConfig::load(&config_path).unwrap();

        assert_eq!(
            config.filters.ignored_profiles,
            vec!["bot@example.com", "ci@example.com"]
        );
        assert!(!config.filters.is_ignored(Some("test@test.com")));
        assert!(config.filters.is_ignored(Some("bot@example.com")));

        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn filters_empty_list_disables_filtering() {
        let temp = temp_dir("filters-empty");
        let config_path = temp.join("config.toml");

        fs::write(
            &config_path,
            r#"[filters]
ignored_profiles = []
"#,
        )
        .unwrap();

        let config = AppConfig::load(&config_path).unwrap();

        assert!(config.filters.ignored_profiles.is_empty());
        assert!(!config.filters.is_ignored(Some("test@test.com")));

        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn filters_is_ignored_trims_and_ignores_case() {
        let filters = super::FiltersConfig {
            ignored_profiles: vec![String::from("  Test@Test.COM  ")],
        };

        assert!(filters.is_ignored(Some("test@test.com")));
        assert!(filters.is_ignored(Some("  TEST@test.com")));
        assert!(!filters.is_ignored(Some("other@test.com")));
        assert!(!filters.is_ignored(None));
        assert!(!filters.is_ignored(Some("   ")));
    }
}
