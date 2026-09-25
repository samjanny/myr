//! Explicit provider transports. API credentials are never read for CLI backends.
use crate::{
    MAX_RESPONSE_BYTES,
    config::{Backend, ProviderConfig},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{io::Read, time::Duration};

const MAX_HTTP_BYTES: usize = 8 * 1024 * 1024;

pub struct Request {
    pub system: String,
    pub prompt: String,
    pub schema: Value,
    pub max_output_tokens: u32,
    pub timeout: Duration,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
}

#[derive(Debug)]
pub struct Completion {
    /// Still untrusted: pass to Session::handle before any semantic use.
    pub raw_output: Vec<u8>,
    pub usage: Usage,
    pub observed_model: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("provider CLI: {0}")]
    Cli(#[from] crate::process::Error),
    #[error("Claude Code must be signed in with a Claude.ai subscription")]
    CliAuth,
    #[error("Codex CLI must be signed in using ChatGPT with an unexpired session")]
    CodexAuth,
    #[error("provider CLI attempted an unexpected native tool ({0})")]
    UnexpectedTool(String),
    #[error("installed CLI lacks required isolation or structured-output flags")]
    CliVersion,
    #[error("provider CLI failed ({0}); no alternate billing backend was attempted")]
    CliExit(String),
    /// The provider itself exhausted its structured-output attempts: an agent
    /// output failure, not provider unavailability.
    #[error("provider exhausted its structured-output attempts")]
    StructuredOutput,
    /// Closed diagnostic for an unparseable CLI event stream.
    #[error("provider CLI returned an unexpected event stream ({0})")]
    CliEvents(String),
    #[error("provider temporary file I/O failed")]
    Io,
    #[error("invalid provider configuration: {0}")]
    Config(#[from] myr_core::ValidationError),
    #[error("API transport cannot execute a subscription backend")]
    WrongTransport,
    #[error("API key environment variable is absent, empty, or invalid")]
    Credentials,
    #[error("provider request exceeds configured limits or has an invalid schema")]
    Request,
    #[error("provider network request failed")]
    Network,
    #[error("provider returned HTTP status {0}")]
    Http(u16),
    #[error("provider response exceeds the size limit")]
    TooLarge,
    #[error("provider returned an incomplete, refused, or malformed structured response")]
    Response,
}

impl Error {
    /// True when the model, not the provider or infrastructure, failed to
    /// produce valid structured output. Runners record INVALID_AGENT_OUTPUT.
    pub fn is_agent_output_failure(&self) -> bool {
        matches!(self, Self::StructuredOutput)
    }
}

/// Closed, sanitized diagnostic token for private attempt records.
pub(crate) fn diagnostic_token(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(64)
        .collect()
}

pub(crate) fn validate_request(request: &Request) -> Result<(), Error> {
    if request.max_output_tokens == 0
        || request.timeout.is_zero()
        || request.timeout > Duration::from_secs(3600)
        || request.prompt.len().saturating_add(request.system.len()) > 2 * MAX_RESPONSE_BYTES
        || request.schema.get("type").and_then(Value::as_str) != Some("object")
    {
        return Err(Error::Request);
    }
    Ok(())
}

/// Execute exactly the configured backend. Errors never trigger another backend.
/// The returned bytes must still pass through the session validation boundary.
pub fn complete(config: &ProviderConfig, request: &Request) -> Result<Completion, Error> {
    match &config.backend {
        Backend::ClaudeCode { .. } => crate::claude_code::complete(config, request),
        Backend::CodexCli { .. } => crate::codex_cli::complete(config, request),
        Backend::OpenaiApi { .. } | Backend::AnthropicApi { .. } => complete_api(config, request),
    }
}

pub fn request_body(config: &ProviderConfig, request: &Request) -> Result<Value, Error> {
    config.validate()?;
    validate_request(request)?;
    match config.backend {
        Backend::OpenaiApi { .. } => Ok(json!({
            "model":config.model,"store":false,"max_output_tokens":request.max_output_tokens,
            "input":[{"role":"system","content":request.system},{"role":"user","content":request.prompt}],
            "text":{"format":{"type":"json_schema","name":"myr_action","strict":true,"schema":request.schema}}
        })),
        Backend::AnthropicApi { .. } => Ok(json!({
            "model":config.model,"system":request.system,"max_tokens":request.max_output_tokens,
            "messages":[{"role":"user","content":request.prompt}],
            "tools":[{"name":"myr_action","description":"Return one Myr action conforming to the supplied schema.","input_schema":request.schema}],
            "tool_choice":{"type":"tool","name":"myr_action","disable_parallel_tool_use":true}
        })),
        _ => Err(Error::WrongTransport),
    }
}

/// Single request, no retry and no fallback. The caller owns budget accounting.
/// Production endpoints are fixed HTTPS origins, with redirects disabled.
pub fn complete_api(config: &ProviderConfig, request: &Request) -> Result<Completion, Error> {
    let body = request_body(config, request)?;
    let (key_env, endpoint) = match &config.backend {
        Backend::OpenaiApi { api_key_env } => (api_key_env, "https://api.openai.com/v1/responses"),
        Backend::AnthropicApi { api_key_env } => {
            (api_key_env, "https://api.anthropic.com/v1/messages")
        }
        _ => return Err(Error::WrongTransport),
    };
    let key = std::env::var(key_env).map_err(|_| Error::Credentials)?;
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(request.timeout)
        .build()
        .map_err(|_| Error::Network)?;
    execute(&client, endpoint, config, &body, &key)
}

fn execute(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    config: &ProviderConfig,
    body: &Value,
    key: &str,
) -> Result<Completion, Error> {
    if key.trim().is_empty() {
        return Err(Error::Credentials);
    }
    let mut secret = reqwest::header::HeaderValue::from_str(key).map_err(|_| Error::Credentials)?;
    secret.set_sensitive(true);
    let request = client.post(endpoint).json(body);
    let request = match config.backend {
        Backend::OpenaiApi { .. } => {
            let mut bearer = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}"))
                .map_err(|_| Error::Credentials)?;
            bearer.set_sensitive(true);
            request.header(reqwest::header::AUTHORIZATION, bearer)
        }
        Backend::AnthropicApi { .. } => request
            .header("x-api-key", secret)
            .header("anthropic-version", "2023-06-01"),
        _ => return Err(Error::WrongTransport),
    };
    let response = request.send().map_err(|_| Error::Network)?;
    if !response.status().is_success() {
        return Err(Error::Http(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_HTTP_BYTES as u64)
    {
        return Err(Error::TooLarge);
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_HTTP_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Network)?;
    parse_response(config, &bytes)
}

pub fn parse_response(config: &ProviderConfig, bytes: &[u8]) -> Result<Completion, Error> {
    if bytes.len() > MAX_HTTP_BYTES {
        return Err(Error::TooLarge);
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| Error::Response)?;
    let (raw_output, usage) = match config.backend {
        Backend::OpenaiApi { .. } => {
            if value["status"] != "completed" || !value["error"].is_null() {
                return Err(Error::Response);
            }
            let mut texts = Vec::new();
            for item in value["output"].as_array().ok_or(Error::Response)? {
                if item["type"] == "reasoning" {
                    continue;
                }
                if item["type"] != "message"
                    || item["role"] != "assistant"
                    || item["status"] != "completed"
                {
                    return Err(Error::Response);
                }
                for content in item["content"].as_array().ok_or(Error::Response)? {
                    if content["type"] != "output_text" {
                        return Err(Error::Response);
                    }
                    texts.push(content["text"].as_str().ok_or(Error::Response)?);
                }
            }
            if texts.len() != 1 {
                return Err(Error::Response);
            }
            (
                texts[0].as_bytes().to_vec(),
                Usage {
                    input_tokens: value["usage"]["input_tokens"].as_u64(),
                    output_tokens: value["usage"]["output_tokens"].as_u64(),
                    cached_input_tokens: value["usage"]["input_tokens_details"]["cached_tokens"]
                        .as_u64(),
                },
            )
        }
        Backend::AnthropicApi { .. } => {
            if value["type"] != "message"
                || value["role"] != "assistant"
                || value["stop_reason"] != "tool_use"
            {
                return Err(Error::Response);
            }
            let mut actions = Vec::new();
            for item in value["content"].as_array().ok_or(Error::Response)? {
                if item["type"] == "text"
                    || item["type"] == "thinking"
                    || item["type"] == "redacted_thinking"
                {
                    continue;
                }
                if item["type"] != "tool_use"
                    || item["name"] != "myr_action"
                    || !item["input"].is_object()
                {
                    return Err(Error::Response);
                }
                actions.push(&item["input"]);
            }
            if actions.len() != 1 {
                return Err(Error::Response);
            }
            (
                serde_json::to_vec(actions[0]).map_err(|_| Error::Response)?,
                Usage {
                    input_tokens: value["usage"]["input_tokens"].as_u64(),
                    output_tokens: value["usage"]["output_tokens"].as_u64(),
                    cached_input_tokens: value["usage"]["cache_read_input_tokens"].as_u64(),
                },
            )
        }
        _ => return Err(Error::WrongTransport),
    };
    if raw_output.len() > MAX_RESPONSE_BYTES {
        return Err(Error::TooLarge);
    }
    Ok(Completion {
        raw_output,
        usage,
        observed_model: value["model"].as_str().map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Role, schema};
    use myr_core::Lineage;
    fn config(anthropic: bool) -> ProviderConfig {
        ProviderConfig {
            backend: if anthropic {
                Backend::AnthropicApi {
                    api_key_env: "TEST_KEY".into(),
                }
            } else {
                Backend::OpenaiApi {
                    api_key_env: "TEST_KEY".into(),
                }
            },
            model: "pinned-model".into(),
            lineage: Lineage {
                provider_id: if anthropic { "anthropic" } else { "openai" }.into(),
                family_id: "test-family".into(),
                checkpoint_id: "pinned-model".into(),
                procedure_id: "test-v0".into(),
            },
        }
    }
    fn response(anthropic: bool) -> Value {
        let action = json!({"action":{"tool":"finish","arguments":{}}});
        if anthropic {
            json!({"type":"message","role":"assistant","model":"pinned-model","stop_reason":"tool_use","content":[{"type":"tool_use","name":"myr_action","input":action}]})
        } else {
            json!({"status":"completed","model":"pinned-model","output":[{"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":action.to_string()}]}]})
        }
    }
    #[test]
    fn both_providers_use_identical_action_schema_and_retain_unknown_usage() {
        let request = Request {
            system: "system".into(),
            prompt: "task".into(),
            schema: schema::response_schema(Role::Worker),
            max_output_tokens: 100,
            timeout: Duration::from_secs(3),
        };
        let a = request_body(&config(false), &request).unwrap();
        let b = request_body(&config(true), &request).unwrap();
        assert_eq!(a["text"]["format"]["schema"], b["tools"][0]["input_schema"]);
        assert_eq!(a["store"], false);
        assert_eq!(b["tool_choice"]["disable_parallel_tool_use"], true);
        let a = parse_response(
            &config(false),
            &serde_json::to_vec(&response(false)).unwrap(),
        )
        .unwrap();
        let b =
            parse_response(&config(true), &serde_json::to_vec(&response(true)).unwrap()).unwrap();
        assert_eq!(a.raw_output, b.raw_output);
        assert_eq!(a.usage, Usage::default());
        assert_eq!(b.usage, Usage::default());
    }
    #[test]
    fn truncation_refusal_multiple_actions_and_wrong_billing_are_rejected() {
        let mut a = response(false);
        a["status"] = "incomplete".into();
        assert!(parse_response(&config(false), &serde_json::to_vec(&a).unwrap()).is_err());
        a = response(false);
        a["output"][0]["content"][0]["type"] = "refusal".into();
        assert!(parse_response(&config(false), &serde_json::to_vec(&a).unwrap()).is_err());
        let mut b = response(true);
        let extra = b["content"][0].clone();
        b["content"].as_array_mut().unwrap().push(extra);
        assert!(parse_response(&config(true), &serde_json::to_vec(&b).unwrap()).is_err());
        b = response(true);
        b["stop_reason"] = "max_tokens".into();
        assert!(parse_response(&config(true), &serde_json::to_vec(&b).unwrap()).is_err());
        let mut cli = config(false);
        cli.backend = Backend::CodexCli {
            executable: "codex".into(),
        };
        assert!(matches!(
            parse_response(&cli, b"{}"),
            Err(Error::WrongTransport)
        ));
    }
    #[test]
    fn http_transport_sends_auth_and_parses_response_without_external_requests() {
        use std::{io::Write, net::TcpListener};
        for anthropic in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let body = response(anthropic).to_string();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut received = Vec::new();
                let mut byte = [0];
                while !received.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).unwrap();
                    received.push(byte[0]);
                }
                let headers = String::from_utf8(received).unwrap().to_lowercase();
                assert!(headers.starts_with("post /fixture "));
                assert!(headers.contains(if anthropic {
                    "x-api-key: fixture-secret"
                } else {
                    "authorization: bearer fixture-secret"
                }));
                let length: usize = headers
                    .lines()
                    .find_map(|s| s.strip_prefix("content-length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                let mut request = vec![0; length];
                socket.read_exact(&mut request).unwrap();
                assert_eq!(request, b"{}");
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            });
            let client = reqwest::blocking::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            let completion = execute(
                &client,
                &format!("http://{address}/fixture"),
                &config(anthropic),
                &json!({}),
                "fixture-secret",
            )
            .unwrap();
            assert!(serde_json::from_slice::<crate::Response>(&completion.raw_output).is_ok());
            server.join().unwrap();
        }
    }
}
