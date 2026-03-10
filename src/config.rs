use std::{fs, io, path::Path};

use serde::Deserialize;

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub ai: AiConfig,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    pub provider: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub cli: AiCliConfig,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AiCliConfig {
    pub prefer: Option<String>,
}

impl AppConfig {
    pub fn load(path: &Path) -> io::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(path)?;
        toml::from_str::<Self>(&contents).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to parse config file {}: {error}", path.display()),
            )
        })
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
            .or_else(|| infer_provider_from_environment())
    }

    pub fn resolved_model(&self) -> Option<String> {
        normalize_value(self.model.as_deref())
    }

    pub fn resolved_api_key(&self) -> Option<String> {
        if let Some(api_key) = normalize_value(self.api_key.as_deref()) {
            return Some(api_key);
        }

        match self.resolved_provider().as_deref() {
            Some("openai") => read_env("DIDDO_OPENAI_KEY"),
            Some("anthropic") => read_env("DIDDO_ANTHROPIC_KEY"),
            _ => None,
        }
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
model = "claude-3-7-sonnet-latest"

[ai.cli]
prefer = "cli"
"#,
        )
        .unwrap();

        let config = AppConfig::load(&config_path).unwrap();

        assert_eq!(config.ai.provider.as_deref(), Some("anthropic"));
        assert_eq!(config.ai.model.as_deref(), Some("claude-3-7-sonnet-latest"));
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

        assert_eq!(config.ai.provider, None);
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
model = "claude-3-7-sonnet-latest"

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

        assert_eq!(config.ai.provider, None);
        assert_eq!(config.ai.model.as_deref(), Some("claude-3-7-sonnet-latest"));
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
model = "  gpt-4.1-mini  "

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
        assert_eq!(config.ai.resolved_model().as_deref(), Some("gpt-4.1-mini"));

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

    fn env_lock() -> &'static Mutex<()> {
        static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

        ENV_LOCK.get_or_init(|| Mutex::new(()))
    }
}
