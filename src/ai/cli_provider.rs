use std::{env, io, path::Path, process::Command};

use crate::{
    ai::{AiError, AiProvider, Result, build_prompt},
    db::Commit,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliTool {
    Claude,
    Codex,
    Opencode,
    CursorAgent,
}

#[allow(dead_code)]
impl CliTool {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "opencode" => Some(Self::Opencode),
            "cursor-agent" | "cursor_agent" => Some(Self::CursorAgent),
            _ => None,
        }
    }

    pub fn binary_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::CursorAgent => "cursor",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::CursorAgent => "cursor-agent",
        }
    }

    pub fn preferred_available(available_tools: &[Self]) -> Option<Self> {
        [Self::Claude, Self::Codex, Self::Opencode, Self::CursorAgent]
            .into_iter()
            .find(|tool| available_tools.contains(tool))
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliProvider {
    tool: CliTool,
}

impl CliProvider {
    pub fn new(tool: CliTool) -> Self {
        Self { tool }
    }

    #[allow(dead_code)]
    fn summarize_with_runner<F>(
        &self,
        commits: &[Commit],
        period: &str,
        mut run_cli: F,
    ) -> Result<String>
    where
        F: FnMut(CliTool, &str) -> io::Result<String>,
    {
        let prompt = build_prompt(commits, period, None);
        let summary = run_cli(self.tool, &prompt)?;
        let trimmed = summary.trim();

        if trimmed.is_empty() {
            return Err(AiError::new(format!(
                "{} returned an empty summary",
                self.tool.display_name()
            )));
        }

        Ok(trimmed.to_string())
    }
}

impl AiProvider for CliProvider {
    fn summarize(&self, commits: &[Commit], period: &str) -> Result<String> {
        self.summarize_with_runner(commits, period, run_cli_command)
    }
}

#[allow(dead_code)]
pub fn detect_installed_tools() -> Vec<CliTool> {
    [CliTool::Claude, CliTool::Codex, CliTool::Opencode, CliTool::CursorAgent]
        .into_iter()
        .filter(|tool| command_exists(tool.binary_name()))
        .collect()
}

fn command_exists(binary: &str) -> bool {
    if binary.contains(std::path::MAIN_SEPARATOR) {
        return is_command_path(Path::new(binary));
    }

    let Some(path_env) = env::var_os("PATH") else {
        return false;
    };

    env::split_paths(&path_env).any(|directory| {
        candidate_binary_names(binary)
            .into_iter()
            .any(|name| is_command_path(&directory.join(name)))
    })
}

fn candidate_binary_names(binary: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        let mut candidates = vec![binary.to_string()];
        let path_ext = env::var_os("PATHEXT")
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string());

        candidates.extend(
            path_ext
                .split(';')
                .filter(|ext| !ext.is_empty())
                .map(|ext| {
                    if binary
                        .to_ascii_lowercase()
                        .ends_with(&ext.to_ascii_lowercase())
                    {
                        binary.to_string()
                    } else {
                        format!("{binary}{ext}")
                    }
                }),
        );

        candidates
    }

    #[cfg(not(windows))]
    {
        vec![binary.to_string()]
    }
}

fn is_command_path(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        return path
            .metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }

    #[cfg(not(unix))]
    {
        true
    }
}

#[allow(dead_code)]
fn run_cli_command(tool: CliTool, prompt: &str) -> io::Result<String> {
    let mut command = Command::new(tool.binary_name());

    match tool {
        CliTool::Claude => {
            command.arg("-p");
            command.arg(prompt);
        }
        CliTool::Codex => {
            command.arg("exec");
            command.arg(prompt);
        }
        CliTool::Opencode => {
            command.arg("run");
            command.arg(prompt);
        }
        CliTool::CursorAgent => {
            command.arg("agent");
            command.arg(prompt);
            command.arg("--no-interactive");
        }
    }

    let output = command.output()?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let message = if stderr.is_empty() {
        format!("{} failed", tool.display_name())
    } else {
        format!("{} failed: {stderr}", tool.display_name())
    };

    Err(io::Error::other(message))
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{CliProvider, CliTool};
    use crate::db::Commit;

    #[test]
    fn trims_cli_output_before_returning_summary() {
        let provider = CliProvider::new(CliTool::Claude);
        let commits = vec![sample_commit()];

        let summary = provider
            .summarize_with_runner(&commits, "today", |tool, prompt| {
                assert_eq!(tool, CliTool::Claude);
                assert!(prompt.contains("feat: add AI summaries"));
                Ok("  concise summary  \n".to_string())
            })
            .unwrap();

        assert_eq!(summary, "concise summary");
    }

    #[test]
    fn rejects_empty_cli_responses() {
        let provider = CliProvider::new(CliTool::Codex);
        let error = provider
            .summarize_with_runner(&[sample_commit()], "today", |_tool, _prompt| {
                Ok("   ".to_string())
            })
            .unwrap_err();

        assert_eq!(error.to_string(), "codex returned an empty summary");
    }

    #[test]
    fn ignores_model_setting_for_cli_execution() {
        let provider = CliProvider::new(CliTool::Claude);
        let summary = provider
            .summarize_with_runner(&[sample_commit()], "today", |_tool, _prompt| {
                Ok("CLI summary".to_string())
            })
            .unwrap();

        assert_eq!(summary, "CLI summary");
    }

    #[test]
    fn implements_ai_provider_trait() {
        let provider = CliProvider::new(CliTool::Codex);
        let summary = provider
            .summarize_with_runner(&[sample_commit()], "week", |_tool, prompt| {
                assert!(prompt.contains("Period: week"));
                Ok("Weekly summary".to_string())
            })
            .unwrap();

        assert_eq!(summary, "Weekly summary");
    }

    fn sample_commit() -> Commit {
        Commit {
            id: None,
            hash: "abc1234".to_string(),
            message: "feat: add AI summaries".to_string(),
            repo_path: "/Users/example/projects/diddo".to_string(),
            repo_name: "diddo".to_string(),
            branch: "feature/diddo".to_string(),
            files_changed: 2,
            insertions: 18,
            deletions: 4,
            committed_at: Utc.with_ymd_and_hms(2026, 3, 10, 9, 15, 0).unwrap(),
        }
    }
}
