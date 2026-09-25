//! Codex CLI subscription transport. Live-provider and sandbox conformance
//! checks are separate acceptance gates, not implied by command construction.
use crate::{
    config::{Backend, ProviderConfig},
    process,
    transport::{Completion, Error, Request, Usage},
};
use serde_json::Value;
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

fn remaining(start: Instant, timeout: Duration) -> Result<Duration, Error> {
    timeout
        .checked_sub(start.elapsed())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| process::Error::Timeout.into())
}
pub fn probe(executable: &Path, timeout: Duration) -> Result<(), Error> {
    let start = Instant::now();
    let dir = tempfile::tempdir().map_err(|_| Error::Io)?;
    let mut help = process::clean_command(executable);
    help.current_dir(dir.path()).args(["exec", "--help"]);
    let output = process::run(help, vec![], remaining(start, timeout)?)?;
    let help = String::from_utf8_lossy(&output.stdout);
    if !output.success
        || [
            "--ignore-user-config",
            "--output-schema",
            "--ephemeral",
            "--json",
            "--strict-config",
        ]
        .iter()
        .any(|f| !help.contains(f))
    {
        return Err(Error::CliVersion);
    }
    let mut auth = process::clean_command(executable);
    auth.current_dir(dir.path()).args(["login", "status"]);
    let output = process::run(auth, vec![], remaining(start, timeout)?)?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.success || !subscription_status(&text) {
        return Err(Error::CodexAuth);
    }
    Ok(())
}
fn subscription_status(text: &str) -> bool {
    text.lines().any(|l| l.trim() == "Logged in using ChatGPT")
        && !text.to_ascii_lowercase().contains("api key")
}
fn command(executable: &Path, dir: &Path, model: &str, output_tokens: u32) -> Command {
    let mut cmd = process::clean_command(executable);
    cmd.current_dir(dir).args([
        "exec",
        "--ignore-user-config",
        "--ignore-rules",
        "--strict-config",
        "--skip-git-repo-check",
        "--ephemeral",
        "--json",
        "--color",
        "never",
        "--model",
        model,
        "--output-schema",
        "schema.json",
    ]);
    for setting in [
        r#"forced_login_method="chatgpt""#,
        r#"model_provider="openai""#,
        r#"approval_policy="never""#,
        r#"default_permissions="myr""#,
        r#"permissions.myr.filesystem={":minimal"="read",":workspace_roots"="read"}"#,
        "permissions.myr.network.enabled=false",
        r#"web_search="disabled""#,
        r#"model_instructions_file="instructions.txt""#,
        "project_doc_max_bytes=0",
        "features.shell_tool=false",
        "features.unified_exec=false",
        "features.multi_agent=false",
        "features.apps=false",
        "features.plugins=false",
        "features.hooks=false",
        "features.memories=false",
        "features.remote_plugin=false",
        "features.shell_snapshot=false",
        "features.skill_mcp_dependency_install=false",
        "features.rollout_budget.enabled=true",
        "features.rollout_budget.prefill_token_weight=0",
        "features.rollout_budget.sampling_token_weight=1",
        // Required by Codex CLI 0.157; an empty list injects no reminder text,
        // so model-visible context stays exactly what Myr accounted for.
        "features.rollout_budget.reminder_at_remaining_tokens=[]",
        // The rollout budget is an under-development feature; its advisory is
        // otherwise emitted as an `error` item that the strict parser rejects.
        "suppress_unstable_features_warning=true",
    ] {
        cmd.args(["-c", setting]);
    }
    cmd.args([
        "-c",
        &format!("features.rollout_budget.limit_tokens={output_tokens}"),
        "-",
    ]);
    cmd
}
pub fn complete(config: &ProviderConfig, request: &Request) -> Result<Completion, Error> {
    config.validate()?;
    crate::transport::validate_request(request)?;
    let Backend::CodexCli { executable } = &config.backend else {
        return Err(Error::WrongTransport);
    };
    let start = Instant::now();
    probe(executable, remaining(start, request.timeout)?)?;
    let dir = tempfile::tempdir().map_err(|_| Error::Io)?;
    std::fs::write(dir.path().join("schema.json"), request.schema.to_string())
        .map_err(|_| Error::Io)?;
    std::fs::write(dir.path().join("instructions.txt"), &request.system).map_err(|_| Error::Io)?;
    let output = process::run(
        command(
            executable,
            dir.path(),
            &config.model,
            request.max_output_tokens,
        ),
        request.prompt.as_bytes().to_vec(),
        remaining(start, request.timeout)?,
    )?;
    if !output.success {
        return Err(if session_rejected(&output.stdout) {
            Error::CodexAuth
        } else if String::from_utf8_lossy(&output.stdout).contains("\"turn.failed\"") {
            Error::CliExit("turn_failed".into())
        } else {
            Error::CliExit("exit".into())
        });
    }
    parse_completion(&output.stdout)
}

/// `codex login status` can report a login whose refresh token has expired;
/// the failed turn then reports an unauthorized response. Never retried.
fn session_rejected(stdout: &[u8]) -> bool {
    String::from_utf8_lossy(stdout).lines().any(|line| {
        serde_json::from_str::<Value>(line).is_ok_and(|event| {
            event["type"] == "turn.failed"
                && event["error"]["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("unauthorized (401)"))
        })
    })
}
pub fn parse_completion(bytes: &[u8]) -> Result<Completion, Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Response)?;
    // `--output-schema` constrains only the final response. Earlier messages in
    // the same turn are commentary: never applied, only the final one is.
    let mut messages: Vec<(Option<String>, Vec<u8>)> = Vec::new();
    let mut usage = None;
    for line in text.lines().filter(|s| !s.trim().is_empty()) {
        let event: Value =
            serde_json::from_str(line).map_err(|_| Error::CliEvents("non_json_line".into()))?;
        match event["type"].as_str() {
            Some("thread.started" | "turn.started") => {}
            Some("item.started" | "item.updated" | "item.completed") => {
                if !matches!(
                    event["item"]["type"].as_str(),
                    Some("reasoning" | "agent_message")
                ) {
                    return Err(Error::UnexpectedTool(crate::transport::diagnostic_token(
                        event["item"]["type"].as_str().unwrap_or("unknown"),
                    )));
                }
                if event["type"] == "item.completed" && event["item"]["type"] == "agent_message" {
                    messages.push((
                        event["item"]["phase"].as_str().map(str::to_owned),
                        event["item"]["text"]
                            .as_str()
                            .ok_or(Error::Response)?
                            .as_bytes()
                            .to_vec(),
                    ));
                }
            }
            Some("turn.completed") => {
                if usage.is_some() {
                    return Err(Error::CliEvents("multiple_turns".into()));
                }
                usage = Some(Usage {
                    input_tokens: event["usage"]["input_tokens"].as_u64(),
                    output_tokens: event["usage"]["output_tokens"].as_u64(),
                    cached_input_tokens: event["usage"]["cached_input_tokens"].as_u64(),
                });
            }
            other => {
                return Err(Error::CliEvents(format!(
                    "event_{}",
                    crate::transport::diagnostic_token(other.unwrap_or("untyped"))
                )));
            }
        }
    }
    let finals: Vec<_> = messages
        .iter()
        .filter(|(phase, _)| phase.as_deref() == Some("final_answer"))
        .collect();
    let raw_output = match finals.as_slice() {
        [(_, text)] => text.clone(),
        [] => messages
            .iter()
            .rev()
            .find(|(phase, _)| phase.as_deref() != Some("commentary"))
            .map(|(_, text)| text.clone())
            .ok_or_else(|| Error::CliEvents("missing_message".into()))?,
        _ => return Err(Error::CliEvents("multiple_final_messages".into())),
    };
    if raw_output.len() > crate::MAX_RESPONSE_BYTES {
        return Err(Error::TooLarge);
    }
    Ok(Completion {
        raw_output,
        usage: usage.ok_or_else(|| Error::CliEvents("missing_usage".into()))?,
        observed_model: None,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_the_final_message_of_a_turn_is_the_response() {
        let message = |text: &str, phase: Option<&str>| {
            let mut item = json!({"type":"agent_message","text":text});
            if let Some(phase) = phase {
                item["phase"] = phase.into();
            }
            json!({"type":"item.completed","item":item}).to_string()
        };
        let turn = |messages: Vec<String>| {
            let mut lines = vec![json!({"type":"turn.started"}).to_string()];
            lines.extend(messages);
            lines.push(
                json!({"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}})
                    .to_string(),
            );
            lines.join("\n")
        };
        let finish = r#"{"action":{"tool":"finish","arguments":{}}}"#;
        // Unmarked commentary precedes the final structured answer.
        let body = turn(vec![
            message("Checking the claim.", None),
            message(finish, None),
        ]);
        assert_eq!(
            parse_completion(body.as_bytes()).unwrap().raw_output,
            finish.as_bytes()
        );
        // An explicit final answer wins even when commentary follows it.
        let body = turn(vec![
            message(finish, Some("final_answer")),
            message("{\"action\":{}}", Some("commentary")),
        ]);
        assert_eq!(
            parse_completion(body.as_bytes()).unwrap().raw_output,
            finish.as_bytes()
        );
        // Two explicit final answers are ambiguous; commentary alone is none.
        let body = turn(vec![
            message(finish, Some("final_answer")),
            message(finish, Some("final_answer")),
        ]);
        assert!(matches!(
            parse_completion(body.as_bytes()),
            Err(Error::CliEvents(_))
        ));
        let body = turn(vec![message("thinking", Some("commentary"))]);
        assert!(matches!(
            parse_completion(body.as_bytes()),
            Err(Error::CliEvents(_))
        ));
    }
    #[test]
    fn expired_session_is_an_authentication_failure() {
        assert!(session_rejected(
            br#"{"type":"error","message":"Reconnecting... 1/5"}
{"type":"turn.failed","error":{"message":"workspace routing discovery unauthorized (401)"}}"#
        ));
        assert!(!session_rejected(
            br#"{"type":"turn.failed","error":{"message":"model overloaded"}}"#
        ));
    }
    #[test]
    fn auth_probe_rejects_api_credentials_and_unknown_status() {
        assert!(subscription_status("Logged in using ChatGPT\n"));
        for s in [
            "Logged in using API key",
            "Not logged in",
            "Logged in using ChatGPT\nAPI key override",
        ] {
            assert!(!subscription_status(s));
        }
    }
    #[test]
    fn complete_turn_required_and_native_tool_events_rejected() {
        let events = [
            json!({"type":"thread.started","thread_id":"fixture"}),
            json!({"type":"item.completed","item":{"type":"agent_message","text":"{\"action\":{\"tool\":\"finish\",\"arguments\":{}}}"}}),
            json!({"type":"turn.completed","usage":{"input_tokens":12,"output_tokens":4}}),
        ];
        let body = events
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let result = parse_completion(body.as_bytes()).unwrap();
        assert_eq!(result.usage.output_tokens, Some(4));
        assert!(result.observed_model.is_none());
        assert!(parse_completion(events[1].to_string().as_bytes()).is_err());
        let tool=json!({"type":"item.completed","item":{"type":"command_execution","command":"not executed by this test"}}).to_string();
        assert!(matches!(
            parse_completion(tool.as_bytes()),
            Err(Error::UnexpectedTool(_))
        ));
    }
    #[test]
    fn command_forces_subscription_and_closed_filesystem_without_bypasses() {
        let command = command(Path::new("codex"), Path::new("."), "pinned-model", 512);
        let args = command
            .get_args()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(args.contains("forced_login_method=\"chatgpt\""));
        assert!(args.contains("--ignore-user-config"));
        assert!(args.contains("default_permissions=\"myr\""));
        assert!(args.contains("permissions.myr.network.enabled=false"));
        assert!(args.contains("features.rollout_budget.reminder_at_remaining_tokens=[]"));
        assert!(args.contains("suppress_unstable_features_warning=true"));
        assert!(!args.contains("dangerously") && !args.contains("danger-full-access"));
    }
}
