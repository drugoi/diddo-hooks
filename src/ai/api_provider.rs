use reqwest::{
    StatusCode,
    blocking::Client,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde_json::{Value, json};

use crate::{
    ai::{AiError, AiProvider, Result, build_prompt},
    config::AiConfig,
    db::Commit,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKind {
    OpenAi,
    Anthropic,
}

#[allow(dead_code)]
impl ApiKind {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "openai" => Some(Self::OpenAi),
            "anthropic" => Some(Self::Anthropic),
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Anthropic => "Anthropic",
        }
    }

    pub(crate) fn default_model(self) -> &'static str {
        match self {
            Self::OpenAi => "gpt-4o-mini",
            Self::Anthropic => "claude-sonnet-4-6",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            Self::OpenAi => "https://api.openai.com/v1/chat/completions",
            Self::Anthropic => "https://api.anthropic.com/v1/messages",
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ApiProvider {
    kind: ApiKind,
    api_key: String,
    model: String,
    prompt_instructions: Option<String>,
    client: Client,
}

impl ApiProvider {
    pub fn from_config(config: &AiConfig, kind: ApiKind) -> Result<Self> {
        let api_key = config
            .resolved_api_key()
            .ok_or_else(|| AiError::new(format!("missing API key for {}", kind.display_name())))?;
        let model = config
            .resolved_model()
            .unwrap_or_else(|| kind.default_model().to_string());
        let prompt_instructions = config.resolved_prompt_instructions().map(String::from);

        Ok(Self::new(kind, api_key, model, prompt_instructions))
    }

    pub fn new(
        kind: ApiKind,
        api_key: String,
        model: String,
        prompt_instructions: Option<String>,
    ) -> Self {
        Self {
            kind,
            api_key,
            model,
            prompt_instructions,
            client: Client::new(),
        }
    }

    #[allow(dead_code)]
    fn summarize_with_client<F>(
        &self,
        commits: &[Commit],
        period: &str,
        mut request: F,
    ) -> Result<String>
    where
        F: FnMut(ApiKind, &str, &str, &str) -> Result<String>,
    {
        let prompt = build_prompt(commits, period, self.prompt_instructions.as_deref());
        request(self.kind, &self.api_key, &self.model, &prompt)
    }
}

impl AiProvider for ApiProvider {
    fn summarize(&self, commits: &[Commit], period: &str) -> Result<String> {
        self.summarize_with_client(commits, period, |kind, api_key, model, prompt| {
            request_summary(&self.client, kind, api_key, model, prompt)
        })
    }
}

#[allow(dead_code)]
fn request_summary(
    client: &Client,
    kind: ApiKind,
    api_key: &str,
    model: &str,
    prompt: &str,
) -> Result<String> {
    let request_body = build_request_body(kind, model, prompt);
    let response = client
        .post(kind.endpoint())
        .headers(build_headers(kind, api_key)?)
        .json(&request_body)
        .send()?;
    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(AiError::new(build_error_message(kind, status, &body)));
    }

    let body: Value = serde_json::from_str(&body).map_err(|error| {
        AiError::new(format!(
            "{} API returned invalid JSON: {error}",
            kind.display_name()
        ))
    })?;

    extract_summary_text(kind, &body).ok_or_else(|| {
        AiError::new(format!(
            "{} API response did not include summary text",
            kind.display_name()
        ))
    })
}

#[allow(dead_code)]
fn build_headers(kind: ApiKind, api_key: &str) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

    match kind {
        ApiKind::OpenAi => {
            let value = HeaderValue::from_str(&format!("Bearer {api_key}"))
                .map_err(|error| AiError::new(error.to_string()))?;
            headers.insert(AUTHORIZATION, value);
        }
        ApiKind::Anthropic => {
            let api_key =
                HeaderValue::from_str(api_key).map_err(|error| AiError::new(error.to_string()))?;
            headers.insert("x-api-key", api_key);
            headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        }
    }

    Ok(headers)
}

#[allow(dead_code)]
fn build_request_body(kind: ApiKind, model: &str, prompt: &str) -> Value {
    match kind {
        ApiKind::OpenAi => json!({
            "model": model,
            "messages": [
                {
                    "role": "system",
                    "content": "You are a concise assistant that summarizes git activity."
                },
                {
                    "role": "user",
                    "content": prompt
                }
            ]
        }),
        ApiKind::Anthropic => json!({
            "model": model,
            "max_tokens": 400,
            "messages": [
                {
                    "role": "user",
                    "content": prompt
                }
            ]
        }),
    }
}

#[allow(dead_code)]
fn extract_summary_text(kind: ApiKind, body: &Value) -> Option<String> {
    let text =
        match kind {
            ApiKind::OpenAi => body
                .get("choices")?
                .as_array()?
                .first()?
                .get("message")?
                .get("content")?
                .as_str()?,
            ApiKind::Anthropic => body.get("content")?.as_array()?.iter().find_map(|part| {
                match part.get("type").and_then(Value::as_str) {
                    Some("text") => part.get("text").and_then(Value::as_str),
                    _ => None,
                }
            })?,
        };

    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[allow(dead_code)]
fn extract_error_message(body: &Value) -> Option<String> {
    if let Some(message) = body
        .get("error")
        .and_then(|error| error.get("message").or(Some(error)))
        .and_then(Value::as_str)
    {
        let trimmed = message.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    body.get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(str::to_string)
}

fn build_error_message(kind: ApiKind, status: StatusCode, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|json| extract_error_message(&json))
        .or_else(|| {
            let trimmed = body.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .unwrap_or_else(|| "unknown API error".to_string());

    format!(
        "{} API request failed ({}): {}",
        kind.display_name(),
        status.as_u16(),
        detail
    )
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use reqwest::StatusCode;
    use serde_json::json;

    use super::{
        ApiKind, ApiProvider, build_error_message, build_request_body, extract_error_message,
        extract_summary_text,
    };
    use crate::db::Commit;

    #[test]
    fn builds_openai_chat_completion_payload() {
        let body = build_request_body(ApiKind::OpenAi, "gpt-4o-mini", "Summarize my work");

        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "Summarize my work");
    }

    #[test]
    fn extracts_summary_text_from_openai_response() {
        let body = json!({
            "choices": [
                {
                    "message": {
                        "content": "  Wrapped up hook integration and config cleanup.  "
                    }
                }
            ]
        });

        let summary = extract_summary_text(ApiKind::OpenAi, &body).unwrap();

        assert_eq!(summary, "Wrapped up hook integration and config cleanup.");
    }

    #[test]
    fn extracts_summary_text_from_anthropic_response() {
        let body = json!({
            "content": [
                { "type": "text", "text": "  Added AI provider selection and prompt building. " }
            ]
        });

        let summary = extract_summary_text(ApiKind::Anthropic, &body).unwrap();

        assert_eq!(summary, "Added AI provider selection and prompt building.");
    }

    #[test]
    fn surfaces_api_error_messages() {
        let body = json!({
            "error": {
                "message": "invalid_api_key"
            }
        });

        assert_eq!(
            extract_error_message(&body).as_deref(),
            Some("invalid_api_key")
        );
    }

    #[test]
    fn preserves_non_json_error_bodies() {
        let message = build_error_message(
            ApiKind::OpenAi,
            StatusCode::BAD_GATEWAY,
            "upstream gateway timeout",
        );

        assert_eq!(
            message,
            "OpenAI API request failed (502): upstream gateway timeout"
        );
    }

    #[test]
    fn passes_prompt_and_model_to_request_callback() {
        let provider = ApiProvider::new(
            ApiKind::Anthropic,
            "anthropic-key".to_string(),
            "claude-sonnet-4-6".to_string(),
            None,
        );
        let summary = provider
            .summarize_with_client(
                &[sample_commit()],
                "week",
                |kind, api_key, model, prompt| {
                    assert_eq!(kind, ApiKind::Anthropic);
                    assert_eq!(api_key, "anthropic-key");
                    assert_eq!(model, "claude-sonnet-4-6");
                    assert!(prompt.contains("Period: week"));
                    Ok("Weekly summary".to_string())
                },
            )
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
            author_email: None,
        }
    }
}
