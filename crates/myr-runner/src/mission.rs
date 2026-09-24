//! User-facing YAML input, before planner IR and goal sealing. Parsing executes
//! nothing; each verifier becomes argv for the configured sandbox executor.
use crate::Result;
use myr_core::{invalid, validate_relative_path};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    goal: String,
    verify: Vec<CommandInput>,
    #[serde(default)]
    protected: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CommandInput {
    Text(String),
    Argv(Vec<String>),
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Mission {
    pub goal: String,
    pub verify: Vec<Vec<String>>,
    pub protected: Vec<String>,
}

/// Bind a planner proposal to the user's original mission before sealing. The
/// planner may add criteria but cannot replace the goal or drop user verifiers.
pub fn compile(
    graph: &mut myr_graph::Graph,
    mission: &Mission,
    ir: crate::goal::GoalIr,
    mut policy: crate::goal::SealPolicy,
) -> Result<crate::goal::SealedGoal> {
    use myr_core::{Argument, Object};
    if ir.goal != mission.goal || mission.verify.is_empty() {
        return Err(invalid(
            "planner goal differs from original mission or has no required verifiers",
        )
        .into());
    }
    let mut accepted_commands = Vec::new();
    for criterion in &ir.criteria {
        if !criterion.binding || !criterion.polarity {
            continue;
        }
        let Object::Atom(atom) = graph.get(criterion.atom)? else {
            continue;
        };
        let Object::PredicateDef(definition) = graph.get(atom.predicate)? else {
            continue;
        };
        if definition.name != "core.command_succeeds" {
            continue;
        }
        let [Argument::Ref(reference)] = atom.arguments.as_slice() else {
            continue;
        };
        if !policy.verifier_policies.contains(reference) {
            continue;
        }
        if let crate::verifier::VerifierPolicy::Command { argv, cwd, .. } =
            crate::verifier::load_policy(graph, *reference, policy.time_budget_ms)?
            && cwd == "."
        {
            accepted_commands.push((*reference, argv));
        }
    }
    let mut used = BTreeSet::new();
    for required in &mission.verify {
        let reference = accepted_commands
            .iter()
            .find(|(r, argv)| argv == required && !used.contains(r))
            .map(|(r, _)| *r)
            .ok_or_else(|| invalid("planner omitted or weakened a required mission verifier"))?;
        used.insert(reference);
    }
    policy.protected.extend(mission.protected.iter().cloned());
    crate::goal::compile(graph, ir, policy)
}

pub fn parse(bytes: &[u8]) -> Result<Mission> {
    if bytes.len() > 1024 * 1024 {
        return Err(invalid("mission YAML exceeds 1 MiB limit").into());
    }
    let input: Input = serde_yaml_ng::from_slice(bytes)
        .map_err(|e| invalid(format!("invalid mission YAML: {e}")))?;
    if input.goal.trim().is_empty() || input.verify.is_empty() {
        return Err(invalid("mission requires a goal and at least one verifier").into());
    }
    let mut verify = Vec::new();
    for command in input.verify {
        let argv = match command {
            CommandInput::Argv(argv) => argv,
            CommandInput::Text(text) => {
                // Do not silently reinterpret shell composition or expansion.
                // Literal metacharacters can always be expressed in argv form.
                if text.contains([';', '&', '|', '<', '>', '`', '$', '\n', '\r']) {
                    return Err(invalid("verifier shorthand cannot contain shell syntax; use explicit argv for literal arguments").into());
                }
                shlex::split(&text)
                    .ok_or_else(|| invalid("unclosed quote or escape in verifier shorthand"))?
            }
        };
        if argv.is_empty() || argv[0].trim().is_empty() || argv.iter().any(|s| s.contains('\0')) {
            return Err(
                invalid("verifier requires nonempty executable and NUL-free arguments").into(),
            );
        }
        verify.push(argv);
    }
    let mut protected = BTreeSet::new();
    let mut portable = BTreeSet::new();
    for path in input.protected {
        let path = path.trim_end_matches('/').to_owned();
        validate_relative_path(&path)?;
        if !portable.insert(path.to_lowercase()) {
            return Err(invalid("duplicate or case-colliding protected paths").into());
        }
        protected.insert(path);
    }
    Ok(Mission {
        goal: input.goal,
        verify,
        protected: protected.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn specification_yaml_and_explicit_arguments_are_preserved() {
        let mission = parse(b"goal: >\n  Replace the cache implementation while preserving existing behavior.\nverify:\n  - cargo test\n  - cargo clippy -- -D warnings\nprotected:\n  - tests/\n  - myr.yaml\n").unwrap();
        assert_eq!(mission.verify[0], ["cargo", "test"]);
        assert_eq!(
            mission.verify[1],
            ["cargo", "clippy", "--", "-D", "warnings"]
        );
        assert_eq!(mission.protected, ["myr.yaml", "tests"]);
        let mission = parse(
            br#"goal: Test literal arguments
verify:
  - ['C:\Program Files\tool.exe', '$HOME', 'a;b', '']
  - 'tool "two words"'
"#,
        )
        .unwrap();
        assert_eq!(
            mission.verify[0],
            ["C:\\Program Files\\tool.exe", "$HOME", "a;b", ""]
        );
        assert_eq!(mission.verify[1], ["tool", "two words"]);
    }
    #[test]
    fn malformed_unknown_ambiguous_and_shell_inputs_are_rejected() {
        for yaml in [
            "goal: x\nverify: [cargo test]\nextra: true",
            "goal: x\ngoal: y\nverify: [cargo test]",
            "goal: x\nverify: []",
            "goal: x\nverify: ['cargo test && echo ok']",
            "goal: x\nverify: ['echo $HOME']",
            "goal: x\nverify: ['tool \"unclosed']",
            "goal: x\nverify: [cargo test]\nprotected: ['../outside']",
            "goal: x\nverify: [cargo test]\nprotected: ['tests', 'Tests/']",
            "goal: x\nverify: [cargo test]\n---\ngoal: y\nverify: [cargo test]",
        ] {
            assert!(parse(yaml.as_bytes()).is_err(), "{yaml}");
        }
    }
}
