//! Official Claude Code subscription transport. No OAuth-token extraction or API
//! credential reuse. CLI availability/auth checks do not call a model.
use crate::{
    config::{Backend, ProviderConfig},
    process,
    transport::{Completion, Error, Request, Usage},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthSummary {
    pub subscription: bool,
    pub method: String,
}

fn remaining(start: Instant, timeout: Duration) -> Result<Duration, Error> {
    timeout
        .checked_sub(start.elapsed())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| process::Error::Timeout.into())
}

fn isolated(executable: &Path, directory: &Path) -> Command {
    let mut command = process::clean_command(executable);
    command.current_dir(directory).args([
        "--safe-mode",
        "--no-chrome",
        "--restricted",
        "--setting-sources",
        "",
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{}}"#,
        "--settings",
        r#"{"forceLoginMethod":"claudeai","disableAllHooks":true,"autoMemoryEnabled":false}"#,
    ]);
    command.env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
    command
}

pub fn parse_auth(bytes: &[u8]) -> Result<AuthSummary, Error> {
    let status: Value = serde_json::from_slice(bytes).map_err(|_| Error::CliAuth)?;
    if status["loggedIn"] != true
        || status["authMethod"] != "claude.ai"
        || status["apiProvider"] != "firstParty"
        || !status["subscriptionType"]
            .as_str()
            .is_some_and(|s| !s.is_empty() && s != "none")
    {
        return Err(Error::CliAuth);
    }
    Ok(AuthSummary {
        subscription: true,
        method: "claude.ai".into(),
    })
}

pub fn probe(executable: &Path, timeout: Duration) -> Result<AuthSummary, Error> {
    let start = Instant::now();
    let directory = tempfile::tempdir().map_err(|_| Error::Io)?;
    let mut help = process::clean_command(executable);
    help.current_dir(directory.path()).arg("--help");
    let output = process::run(help, vec![], remaining(start, timeout)?)?;
    let help = String::from_utf8_lossy(&output.stdout);
    if !output.success
        || [
            "--safe-mode",
            "--no-chrome",
            "--restricted",
            "--tools",
            "--strict-mcp-config",
            "--setting-sources",
            "--no-session-persistence",
            "--json-schema",
            "--disable-slash-commands",
            "--permission-prompts",
        ]
        .iter()
        .any(|flag| !help.contains(flag))
    {
        return Err(Error::CliVersion);
    }
    let mut auth = isolated(executable, directory.path());
    auth.args(["auth", "status", "--json"]);
    let output = process::run(auth, vec![], remaining(start, timeout)?)?;
    if !output.success {
        return Err(Error::CliAuth);
    }
    parse_auth(&output.stdout)
}

pub fn complete(config: &ProviderConfig, request: &Request) -> Result<Completion, Error> {
    config.validate()?;
    crate::transport::validate_request(request)?;
    let Backend::ClaudeCode { executable } = &config.backend else {
        return Err(Error::WrongTransport);
    };
    let start = Instant::now();
    probe(executable, remaining(start, request.timeout)?)?;
    let schema = request.schema.to_string();
    // Keep below Windows' command-line limit after quoting. Prompt bodies travel
    // through stdin, never shell interpolation or command-line arguments.
    if schema.len().saturating_add(request.system.len()) > 12000 {
        return Err(Error::Request);
    }
    let directory = tempfile::tempdir().map_err(|_| Error::Io)?;
    let mut command = isolated(executable, directory.path());
    command.args([
        "--print",
        "--output-format",
        "json",
        "--json-schema",
        &schema,
        "--model",
        &config.model,
        "--system-prompt",
        &request.system,
        "--tools",
        "",
        "--disable-slash-commands",
        "--no-session-persistence",
        "--permission-mode",
        "dontAsk",
        "--permission-prompts",
        "none",
    ]);
    command.env(
        "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
        request.max_output_tokens.to_string(),
    );
    let output = process::run(
        command,
        request.prompt.as_bytes().to_vec(),
        remaining(start, request.timeout)?,
    )?;
    if !output.success {
        return Err(Error::CliExit);
    }
    parse_completion(&output.stdout)
}

pub fn parse_completion(bytes: &[u8]) -> Result<Completion, Error> {
    let result: Value = serde_json::from_slice(bytes).map_err(|_| Error::Response)?;
    if result["type"] != "result"
        || result["subtype"] != "success"
        || result["is_error"] != false
        || !result["structured_output"].is_object()
    {
        return Err(Error::Response);
    }
    let raw_output =
        serde_json::to_vec(&result["structured_output"]).map_err(|_| Error::Response)?;
    if raw_output.len() > crate::MAX_RESPONSE_BYTES {
        return Err(Error::TooLarge);
    }
    let observed_model = result["modelUsage"]
        .as_object()
        .filter(|m| m.len() == 1)
        .and_then(|m| m.keys().next().cloned());
    Ok(Completion {
        raw_output,
        observed_model,
        usage: Usage {
            input_tokens: result["usage"]["input_tokens"].as_u64(),
            output_tokens: result["usage"]["output_tokens"].as_u64(),
            cached_input_tokens: result["usage"]["cache_read_input_tokens"].as_u64(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_subscription_auth_is_accepted_and_account_details_are_not_returned() {
        let valid = json!({"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"max","email":"private@example.invalid"});
        let summary = parse_auth(&serde_json::to_vec(&valid).unwrap()).unwrap();
        assert!(summary.subscription);
        assert!(!serde_json::to_string(&summary).unwrap().contains("private"));
        for (key, value) in [
            ("loggedIn", json!(false)),
            ("authMethod", json!("api_key")),
            ("apiProvider", json!("bedrock")),
            ("subscriptionType", Value::Null),
        ] {
            let mut invalid = valid.clone();
            invalid[key] = value;
            assert!(parse_auth(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
    }
    #[test]
    fn plain_prose_and_error_results_never_enter_structured_channel() {
        let result = json!({"type":"result","subtype":"success","is_error":false,"structured_output":{"action":{"tool":"finish","arguments":{}}},"modelUsage":{"observed-checkpoint":{}},"usage":{"input_tokens":7,"output_tokens":3}});
        let completion = parse_completion(&serde_json::to_vec(&result).unwrap()).unwrap();
        assert_eq!(
            completion.observed_model.as_deref(),
            Some("observed-checkpoint")
        );
        assert_eq!(completion.usage.input_tokens, Some(7));
        let mut error = result.clone();
        error["is_error"] = true.into();
        assert!(parse_completion(&serde_json::to_vec(&error).unwrap()).is_err());
        let mut prose = result;
        prose.as_object_mut().unwrap().remove("structured_output");
        prose["result"] = "looks good".into();
        assert!(parse_completion(&serde_json::to_vec(&prose).unwrap()).is_err());
    }
    #[test]
    fn native_tools_mcp_and_config_sources_are_disabled_without_bare_mode() {
        let command = isolated(Path::new("claude"), Path::new("."));
        let args: Vec<_> = command
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|s| s == "--restricted"));
        assert!(args.iter().any(|s| s == "--safe-mode"));
        assert!(args.iter().any(|s| s == "--strict-mcp-config"));
        assert!(args.iter().any(|s| s.contains("forceLoginMethod")));
        assert!(!args.iter().any(|s| s == "--bare"));
    }
}
