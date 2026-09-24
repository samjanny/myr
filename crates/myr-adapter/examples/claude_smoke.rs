//! Explicit live subscription smoke test. Running this consumes CLI plan usage.
use myr_adapter::{
    Role, claude_code,
    config::{Backend, ProviderConfig},
    schema,
    transport::Request,
};
use myr_core::Lineage;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let executable = args
        .next()
        .ok_or("usage: claude_smoke <executable> <model>")?;
    let model = args
        .next()
        .ok_or("model is required")?
        .into_string()
        .map_err(|_| "invalid model")?;
    let config = ProviderConfig {
        backend: Backend::ClaudeCode {
            executable: executable.into(),
        },
        model: model.clone(),
        lineage: Lineage {
            provider_id: "anthropic".into(),
            family_id: "claude".into(),
            checkpoint_id: model,
            procedure_id: "smoke-v0".into(),
        },
    };
    let request=Request {system:"You return one Myr action matching the supplied schema. Do not use native tools.".into(),prompt:"Return the finish action with an empty arguments object. This is a transport smoke test.".into(),schema:schema::response_schema(Role::Worker),max_output_tokens:512,timeout:std::time::Duration::from_secs(60)};
    let result = claude_code::complete(&config, &request)?;
    let action: myr_adapter::Response = serde_json::from_slice(&result.raw_output)?;
    if !matches!(action.action, myr_adapter::Action::Finish {}) {
        return Err("unexpected smoke-test action".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"billing":"subscription","action":action,"usage":result.usage,"observed_model":result.observed_model})
        )?
    );
    Ok(())
}
