//! Pre-seal planner contract. Only the runtime supplies the catalog and policy;
//! model proposals must pass mission binding and the existing goal compiler.
use crate::{
    Result,
    goal::{GoalIr, SealPolicy, SealedGoal},
    mission::{self, Mission},
};
use myr_core::*;
use myr_graph::{Authority, Graph};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Reference sets up to this size are enumerated in the planner schema; larger
/// sets use the CID pattern so the schema stays bounded for big repositories.
/// Access control never depends on the schema: the catalog checks every action.
pub const MAX_ENUMERATED_REFERENCES: usize = 16;

pub struct Catalog {
    registry: BTreeSet<ObjectRef>,
    atoms: BTreeSet<ObjectRef>,
    artifacts: BTreeSet<ObjectRef>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    action: Action,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "tool", content = "arguments", deny_unknown_fields)]
enum Action {
    #[serde(rename = "submit_goal")]
    Submit(GoalIr),
    #[serde(rename = "define_atom")]
    DefineAtom {
        predicate_ref: ObjectRef,
        arguments: Vec<Argument>,
    },
    #[serde(rename = "put_rationale")]
    PutRationale { text: String },
    #[serde(rename = "fetch")]
    Fetch { reference: ObjectRef },
}

pub enum Step {
    Created(ObjectRef),
    Fetched {
        reference: ObjectRef,
        bytes: Vec<u8>,
    },
    Sealed(Box<SealedGoal>),
}

fn decode(raw: &[u8]) -> Result<Action> {
    if raw.len() > 1024 * 1024 {
        return Err(invalid("planner output exceeds 1 MiB").into());
    }
    let response: Response = serde_json::from_slice(raw)?;
    if serde_json::to_value(&response)? != serde_json::from_slice::<Value>(raw)? {
        return Err(invalid("planner output omits required contract fields").into());
    }
    Ok(response.action)
}

fn record(properties: Value) -> Value {
    let required: Vec<_> = properties
        .as_object()
        .expect("schema properties")
        .keys()
        .cloned()
        .collect();
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn references(kind: Kind, allowed: &BTreeSet<ObjectRef>) -> Value {
    let cid = if allowed.is_empty() || allowed.len() > MAX_ENUMERATED_REFERENCES {
        json!({"type":"string","pattern":"^b3:[0-9a-f]{64}$"})
    } else {
        json!({"type":"string","enum":allowed.iter().map(|r| r.cid.to_string()).collect::<Vec<_>>()})
    };
    record(json!({"kind":{"type":"string","enum":[kind]},"cid":cid}))
}
fn list(items: Value, empty_only: bool) -> Value {
    let mut value = json!({"type":"array","items":items});
    if empty_only {
        value["maxItems"] = json!(0);
    }
    value
}

impl Catalog {
    /// Catalog contents are explicitly selected worker-visible references, not a
    /// global CAS listing. Creation validates liveness and predicate membership.
    pub fn new(
        graph: &Graph,
        registry: &[ObjectRef],
        atoms: &[ObjectRef],
        artifacts: &[ObjectRef],
    ) -> Result<Self> {
        let make = |values: &[ObjectRef], kind| -> Result<BTreeSet<ObjectRef>> {
            let mut set = BTreeSet::new();
            for reference in values {
                reference.require(kind)?;
                if !set.insert(*reference) || !graph.live(*reference)? {
                    return Err(invalid("duplicate or inactive planner catalog reference").into());
                }
            }
            Ok(set)
        };
        let catalog = Self {
            registry: make(registry, Kind::PredicateDef)?,
            atoms: make(atoms, Kind::Atom)?,
            artifacts: make(artifacts, Kind::Artifact)?,
        };
        if catalog.registry.is_empty() {
            return Err(invalid("planner catalog requires registered predicates").into());
        }
        catalog.revalidate(graph)?;
        Ok(catalog)
    }

    fn contains(&self, reference: ObjectRef) -> bool {
        self.registry.contains(&reference)
            || self.atoms.contains(&reference)
            || self.artifacts.contains(&reference)
    }

    /// Expand only through registered predicates and task-local references.
    /// Graph insertion validates argument arity/types before any node is indexed.
    pub fn define_atom(
        &mut self,
        graph: &mut Graph,
        predicate: ObjectRef,
        arguments: Vec<Argument>,
    ) -> Result<ObjectRef> {
        self.revalidate(graph)?;
        if !self.registry.contains(&predicate)
            || arguments
                .iter()
                .any(|a| matches!(a, Argument::Ref(r) if !self.contains(*r)))
        {
            return Err(invalid("planner ATOM references objects outside its catalog").into());
        }
        let reference = graph.insert(
            Authority::Worker,
            &Object::Atom(Atom {
                predicate,
                arguments,
            }),
        )?;
        self.atoms.insert(reference);
        Ok(reference)
    }

    /// Only text rationale creation is exposed before sealing; never registry
    /// mutation, arbitrary filesystem writes, provider changes, or FACT creation.
    pub fn handle_response(
        &mut self,
        graph: &mut Graph,
        mission: &Mission,
        policy: &SealPolicy,
        raw: &[u8],
    ) -> Result<Step> {
        self.revalidate(graph)?;
        match decode(raw)? {
            Action::DefineAtom {
                predicate_ref,
                arguments,
            } => Ok(Step::Created(self.define_atom(
                graph,
                predicate_ref,
                arguments,
            )?)),
            Action::PutRationale { text } => {
                if text.trim().is_empty() || text.len() > myr_adapter::MAX_ARTIFACT_BYTES {
                    return Err(invalid("planner rationale is empty or too large").into());
                }
                let reference = graph.register_artifact(text.as_bytes())?;
                self.artifacts.insert(reference);
                Ok(Step::Created(reference))
            }
            Action::Fetch { reference } => {
                if !self.contains(reference) {
                    return Err(invalid("planner fetch is outside its catalog").into());
                }
                Ok(Step::Fetched {
                    reference,
                    bytes: graph.cas().get(reference)?,
                })
            }
            Action::Submit(ir) => Ok(Step::Sealed(Box::new(self.compile_ir(
                graph,
                mission,
                policy.clone(),
                ir,
            )?))),
        }
    }

    pub(crate) fn revalidate(&self, graph: &Graph) -> Result<()> {
        for reference in self
            .registry
            .iter()
            .chain(&self.atoms)
            .chain(&self.artifacts)
        {
            if !graph.live(*reference)? {
                return Err(invalid("planner catalog reference became inactive").into());
            }
        }
        for reference in &self.atoms {
            let Object::Atom(atom) = graph.get(*reference)? else {
                return Err(invalid("planner atom has wrong kind").into());
            };
            if !self.registry.contains(&atom.predicate) {
                return Err(invalid("planner atom predicate is outside catalog registry").into());
            }
        }
        Ok(())
    }

    pub(crate) fn description(&self, graph: &Graph) -> Result<Value> {
        self.revalidate(graph)?;
        let objects = self.registry.iter().chain(&self.atoms).map(|reference| {
            Ok(json!({"reference":reference,"object":serde_json::from_str::<Value>(&myr_wire::render(&graph.get(*reference)?)?)?}))
        }).collect::<Result<Vec<_>>>()?;
        Ok(json!({"objects":objects,"artifacts":self.artifacts}))
    }

    /// Separate capability checks from repairable schema/semantic mistakes.
    pub(crate) fn access_allowed(&self, raw: &[u8]) -> Result<bool> {
        Ok(match decode(raw)? {
            Action::Fetch { reference } => self.contains(reference),
            Action::DefineAtom {
                predicate_ref,
                arguments,
            } => {
                self.registry.contains(&predicate_ref)
                    && arguments
                        .iter()
                        .all(|a| !matches!(a, Argument::Ref(r) if !self.contains(*r)))
            }
            Action::PutRationale { .. } => true,
            Action::Submit(ir) => self.ir_allowed(&ir),
        })
    }

    fn ir_allowed(&self, ir: &GoalIr) -> bool {
        ir.registry.iter().all(|r| self.registry.contains(r))
            && ir.criteria.iter().all(|c| {
                self.atoms.contains(&c.atom)
                    && c.measurement
                        .as_ref()
                        .is_none_or(|m| self.artifacts.contains(&m.procedure))
            })
            && ir.assumptions.iter().all(|a| {
                self.atoms.contains(&a.atom)
                    && self.artifacts.contains(&a.rationale)
                    && a.artifacts.iter().all(|r| self.artifacts.contains(r))
            })
    }

    /// Compatible action envelope for the four existing transports and schema
    /// accounting. This contract does not add an agent API for FACT or EVIDENCE.
    pub fn response_schema(&self, mission: &Mission) -> Value {
        let atom = references(Kind::Atom, &self.atoms);
        let artifact = references(Kind::Artifact, &self.artifacts);
        let measurement = if self.artifacts.is_empty() {
            json!({"type":"null"})
        } else {
            json!({"anyOf":[{"type":"null"},record(json!({"procedure":artifact,
                "tolerance":record(json!({"mantissa":{"type":"integer"},"exponent":{"type":"integer"}}))}))]})
        };
        let criterion = record(
            json!({"atom":atom,"polarity":{"type":"boolean"},"binding":{"type":"boolean"},"measurement":measurement}),
        );
        let assumption = record(
            json!({"atom":atom,"question":{"type":"string"},"chosen":{"type":"string"},
            "alternatives":list(json!({"type":"string"}),false),"rationale":artifact,
            "artifacts":list(artifact.clone(),self.artifacts.is_empty())}),
        );
        let ir = record(json!({"goal":{"type":"string","enum":[mission.goal]},
            "registry":list(references(Kind::PredicateDef,&self.registry),false),
            "criteria":list(criterion,self.atoms.is_empty()),"assumptions":list(assumption,self.artifacts.is_empty() || self.atoms.is_empty())}));
        let action =
            record(json!({"tool":{"type":"string","enum":["submit_goal"]},"arguments":ir}));
        let mut ref_variants = vec![references(Kind::PredicateDef, &self.registry)];
        if !self.atoms.is_empty() {
            ref_variants.push(atom);
        }
        if !self.artifacts.is_empty() {
            ref_variants.push(artifact);
        }
        let reference = json!({"anyOf":ref_variants});
        let argument_variants: Vec<_> = [
            ("ref", reference.clone()),
            ("text", json!({"type":"string"})),
            ("integer", json!({"type":"integer"})),
            ("boolean", json!({"type":"boolean"})),
            (
                "decimal",
                record(json!({"mantissa":{"type":"integer"},"exponent":{"type":"integer"}})),
            ),
        ]
        .into_iter()
        .map(|(tag, value)| record(json!({"type":{"type":"string","enum":[tag]},"value":value})))
        .collect();
        let define = record(json!({"tool":{"type":"string","enum":["define_atom"]},
            "arguments":record(json!({"predicate_ref":references(Kind::PredicateDef,&self.registry),"arguments":list(json!({"anyOf":argument_variants}),false)}))}));
        let rationale = record(
            json!({"tool":{"type":"string","enum":["put_rationale"]},"arguments":record(json!({"text":{"type":"string"}}))}),
        );
        let fetch = record(
            json!({"tool":{"type":"string","enum":["fetch"]},"arguments":record(json!({"reference":reference}))}),
        );
        record(json!({"action":{"anyOf":[action,define,rationale,fetch]}}))
    }

    /// Read and validate untrusted bytes before the compiler writes a goal seal.
    /// No provider, billing, baseline or verifier-policy field is model-owned.
    pub fn compile_response(
        &self,
        graph: &mut Graph,
        mission: &Mission,
        policy: SealPolicy,
        raw: &[u8],
    ) -> Result<SealedGoal> {
        self.revalidate(graph)?;
        let Action::Submit(ir) = decode(raw)? else {
            return Err(invalid("submit_goal action required for compilation").into());
        };
        self.compile_ir(graph, mission, policy, ir)
    }

    fn compile_ir(
        &self,
        graph: &mut Graph,
        mission: &Mission,
        policy: SealPolicy,
        ir: GoalIr,
    ) -> Result<SealedGoal> {
        if !self.ir_allowed(&ir) {
            return Err(invalid("planner proposal references objects outside its catalog").into());
        }
        mission::compile(graph, mission, ir, policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{goal, mission::Mission};

    #[test]
    fn large_catalogs_keep_the_planner_schema_bounded() {
        let mut f = crate::command_verification::tests::Fixture::new(true);
        let sealed = goal::load(&f.graph, f.goal).unwrap();
        let mission = Mission {
            goal: sealed.ir().goal.clone(),
            verify: vec![vec!["check".into()]],
            protected: vec![],
        };
        let mut artifacts = vec![f.policy];
        for index in 0..MAX_ENUMERATED_REFERENCES {
            artifacts.push(
                f.graph
                    .register_artifact(format!("file {index}").as_bytes())
                    .unwrap(),
            );
        }
        let small = Catalog::new(
            &f.graph,
            &sealed.ir().registry,
            &[f.atom],
            &artifacts[..MAX_ENUMERATED_REFERENCES],
        )
        .unwrap();
        let enumerated = small.response_schema(&mission).to_string();
        assert!(enumerated.contains(&f.policy.cid.to_string()));
        let large = Catalog::new(&f.graph, &sealed.ir().registry, &[f.atom], &artifacts).unwrap();
        let bounded = large.response_schema(&mission).to_string();
        assert!(!bounded.contains(&artifacts[1].cid.to_string()));
        assert!(bounded.contains(&f.atom.cid.to_string()));
        // Claude Code passes the schema on the command line; keep it well below
        // that transport's documented limit even for repositories with many files.
        for index in 0..2000 {
            artifacts.push(
                f.graph
                    .register_artifact(format!("large file {index}").as_bytes())
                    .unwrap(),
            );
        }
        let huge = Catalog::new(&f.graph, &sealed.ir().registry, &[f.atom], &artifacts).unwrap();
        assert!(huge.response_schema(&mission).to_string().len() < 8000);
    }
}
