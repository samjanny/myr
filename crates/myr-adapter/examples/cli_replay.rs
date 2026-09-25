//! Replay one recorded request through a subscription CLI backend, for live
//! transport diagnosis. Running this consumes CLI plan usage; API backends are
//! refused. Inputs are files reconstructed from a private dispatch journal.
//!
//! cli_replay <claude-code|codex-cli> <executable> <model> <dir> [repetitions]
//! where <dir> holds instructions.txt, prompt.txt and schema.json.
use myr_adapter::{
    config::{Backend, ProviderConfig},
    transport::{self, Request},
};
use myr_core::Lineage;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [kind, executable, model, dir, rest @ ..] = args.as_slice() else {
        return Err(
            "usage: cli_replay <claude-code|codex-cli> <executable> <model> <dir> [n]".into(),
        );
    };
    let repetitions: u32 = rest.first().map(|n| n.parse()).transpose()?.unwrap_or(1);
    let (backend, provider, family) = match kind.as_str() {
        "claude-code" => (
            Backend::ClaudeCode {
                executable: executable.into(),
            },
            "anthropic",
            "claude",
        ),
        "codex-cli" => (
            Backend::CodexCli {
                executable: executable.into(),
            },
            "openai",
            "gpt",
        ),
        _ => return Err("only subscription CLI backends can be replayed".into()),
    };
    let config = ProviderConfig {
        backend,
        model: model.clone(),
        lineage: Lineage {
            provider_id: provider.into(),
            family_id: family.into(),
            checkpoint_id: model.clone(),
            procedure_id: "replay-v0".into(),
        },
    };
    let dir = std::path::Path::new(dir);
    let request = Request {
        system: std::fs::read_to_string(dir.join("instructions.txt"))?,
        prompt: std::fs::read_to_string(dir.join("prompt.txt"))?,
        schema: serde_json::from_slice(&std::fs::read(dir.join("schema.json"))?)?,
        max_output_tokens: 8192,
        timeout: std::time::Duration::from_secs(600),
    };
    for attempt in 1..=repetitions {
        let started = std::time::Instant::now();
        let outcome = match transport::complete(&config, &request) {
            Ok(completion) => serde_json::json!({"ok":true,
                "action":serde_json::from_slice::<serde_json::Value>(&completion.raw_output)
                    .map(|v| v["action"]["tool"].clone()).unwrap_or_default(),
                "usage":completion.usage}),
            Err(error) => serde_json::json!({"ok":false,"error":error.to_string()}),
        };
        println!(
            "{}",
            serde_json::json!({"attempt":attempt,"seconds":started.elapsed().as_secs(),"outcome":outcome})
        );
    }
    Ok(())
}
