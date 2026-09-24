use base64::{Engine, engine::general_purpose::STANDARD};
use clap::{Parser, Subcommand};
use myr_core::{Cid, Kind, Object};
use myr_graph::{Authority, Graph};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "myr",
    version,
    about = "Typed agent communication and verified knowledge",
    long_about = "Myr v0 development CLI. Run uses explicitly configured subscription or API providers. Pilot uses simulated agents. Live sandbox conformance and benchmark acceptance remain incomplete."
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Join receipts to a benchmark plan and report missing or invalid pairs.
    CollectBenchmark { input: PathBuf },
    /// Plan randomized paired benchmark jobs without running them.
    PlanBenchmark { input: PathBuf },
    /// Analyze complete paired benchmark measurements; not official acceptance.
    AnalyzeBenchmark { input: PathBuf },
    /// Export a live candidate into a new directory without executing it.
    ExportCandidate {
        cid: Cid,
        output: PathBuf,
        #[arg(long, default_value = ".myr")]
        root: PathBuf,
    },
    /// Plan and execute a mission using explicitly configured providers.
    Run {
        mission: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = ".")]
        repository: PathBuf,
        #[arg(long, default_value = ".myr")]
        root: PathBuf,
        /// Prepare the snapshot, catalog and policy without provider or tool calls.
        #[arg(long)]
        prepare_only: bool,
    },
    /// Import a repository baseline into a new store without invoking agents.
    Snapshot {
        repository: PathBuf,
        #[arg(long, default_value = ".myr")]
        root: PathBuf,
        #[arg(long)]
        read_policy: Option<PathBuf>,
    },
    /// Validate mission YAML and print normalized argv without executing it.
    CheckMission { file: PathBuf },
    /// Validate a Goal IR JSON file and policy JSON against an existing store.
    Seal {
        #[arg(long)]
        ir: PathBuf,
        #[arg(long)]
        policy: PathBuf,
        /// Bind the IR to the original YAML goal, required checks and protections.
        #[arg(long)]
        mission: Option<PathBuf>,
        #[arg(long, default_value = ".myr")]
        root: PathBuf,
    },
    /// Reopen and validate a sealed goal, including current dependencies.
    InspectGoal {
        cid: Cid,
        #[arg(long, default_value = ".myr")]
        root: PathBuf,
    },
    /// Run the development-only fixture pilot without making model calls.
    Pilot {
        /// A new directory for immutable pilot evidence.
        output: PathBuf,
    },
    /// Inspect a stored object, including inactive historical objects.
    Show {
        cid: Cid,
        #[arg(long, default_value = ".myr")]
        root: PathBuf,
    },
    /// Append an assumption invalidation and mark its dependents stale.
    Invalidate {
        cid: Cid,
        #[arg(long, default_value = ".myr")]
        root: PathBuf,
        #[arg(long)]
        reason: String,
    },
}

fn open_existing(root: &Path) -> Result<Graph, Box<dyn std::error::Error>> {
    if !root.join("graph.sqlite").is_file() || !root.join("objects").is_dir() {
        return Err("store does not exist; expected graph.sqlite and objects/".into());
    }
    Ok(Graph::open(
        root.join("graph.sqlite"),
        myr_cas::Store::open(root.join("objects"))?,
    )?)
}

fn execute(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let mut mission_failed = false;
    let result = match args.command {
        Command::CollectBenchmark { input } => {
            let input = serde_json::from_slice(&read_config(&input)?)?;
            serde_json::to_value(myr_runner::collection::collect(&input)?)?
        }
        Command::PlanBenchmark { input } => {
            let input = serde_json::from_slice(&read_config(&input)?)?;
            serde_json::to_value(myr_runner::schedule::plan(&input)?)?
        }
        Command::AnalyzeBenchmark { input } => {
            let input = serde_json::from_slice(&read_config(&input)?)?;
            serde_json::to_value(myr_runner::statistics::analyze(&input)?)?
        }
        Command::ExportCandidate { cid, output, root } => {
            let graph = open_existing(&root)?;
            let parent = output
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            if std::fs::canonicalize(parent)?.starts_with(std::fs::canonicalize(&root)?) {
                return Err("candidate output must be outside the mission store".into());
            }
            let reference = graph.resolve(cid)?;
            let manifest = myr_runner::candidate::export(&graph, reference, &output)?;
            serde_json::json!({"exported":true,"candidate":reference,"output":std::fs::canonicalize(&output)?,"files":manifest.files,"mission_completion_checked":false})
        }
        Command::Run {
            mission,
            config,
            repository,
            root,
            prepare_only,
        } => 'run: {
            if root.exists() {
                return Err("run output store must be a new directory".into());
            }
            let mission_bytes = read_config(&mission)?;
            let mission = match myr_runner::mission::parse(&mission_bytes) {
                Ok(mission) => mission,
                Err(myr_runner::Error::Validation(_)) => {
                    let mut graph = create_run_store(&root)?;
                    let audit = myr_cas::Store::open(root.join("private-audit"))?;
                    let rejected =
                        myr_runner::admission::reject_mission(&mut graph, &audit, &mission_bytes)?;
                    let mut value = serde_json::to_value(rejected.report)?;
                    value["private_record"] = serde_json::to_value(rejected.private_record)?;
                    std::fs::write(root.join("result.json"), serde_json::to_vec_pretty(&value)?)?;
                    mission_failed = true;
                    break 'run value;
                }
                Err(error) => return Err(error.into()),
            };
            let config: myr_runner::setup::Config = serde_json::from_slice(&read_config(&config)?)?;
            config.validate(&mission)?;
            let tree = myr_runner::snapshot::read(&repository, &config.read_policy)?;
            let mut graph = create_run_store(&root)?;
            let mut prepared = myr_runner::setup::prepare(
                &mut graph,
                &mission,
                &tree,
                &config,
                &root.join("quarantine"),
            )?;
            let preparation = serde_json::json!({"format":"myr-preparation-v0","mission":mission,"configuration":prepared.configuration,
                "policy":prepared.policy,"measurement_procedures":prepared.measurement_procedures,"planner_schema":prepared.catalog.response_schema(&mission)});
            std::fs::write(
                root.join("prepared.json"),
                serde_json::to_vec_pretty(&preparation)?,
            )?;
            if prepare_only {
                serde_json::json!({"prepared":true,"executed":false,"root":root,"configuration":prepared.configuration,"baseline":prepared.policy.baseline})
            } else {
                let audit = myr_cas::Store::open(root.join("private-audit"))?;
                let outcome = myr_runner::pipeline::plan_and_run(
                    &mut graph,
                    &audit,
                    &mission,
                    &prepared.policy,
                    &mut prepared.catalog,
                    &prepared.execution,
                )?;
                let value = match outcome {
                    myr_runner::pipeline::MissionOutcome::Executed {
                        planning_created,
                        outcome,
                    } => {
                        mission_failed = !matches!(
                            outcome.report.state,
                            myr_core::MissionState::Complete
                                | myr_core::MissionState::CompleteWithAssumptions
                        );
                        serde_json::json!({"planning_created":planning_created,"private_record":outcome.private_record,"report":outcome.report})
                    }
                    myr_runner::pipeline::MissionOutcome::PlanningFailed {
                        report,
                        private_record,
                    } => {
                        mission_failed = true;
                        let mut value = serde_json::to_value(report)?;
                        value["private_record"] = serde_json::to_value(private_record)?;
                        value
                    }
                };
                std::fs::write(root.join("result.json"), serde_json::to_vec_pretty(&value)?)?;
                value
            }
        }
        Command::Snapshot {
            repository,
            root,
            read_policy,
        } => {
            if root.exists() {
                return Err("snapshot output store must be a new directory".into());
            }
            let policy = if let Some(path) = read_policy {
                serde_json::from_slice(&read_config(&path)?)?
            } else {
                myr_runner::snapshot::ReadPolicy::default()
            };
            let tree = myr_runner::snapshot::read(&repository, &policy)?;
            if let Some(parent) = root.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::create_dir(&root)?;
            let mut graph = Graph::open(
                root.join("graph.sqlite"),
                myr_cas::Store::open(root.join("objects"))?,
            )?;
            let (snapshot, files) = myr_runner::views::import_worker(&mut graph, &tree)?;
            let provenance = graph.register_artifact(&serde_json::to_vec(
                &serde_json::json!({"snapshot":snapshot,"read_policy":policy}),
            )?)?;
            graph.depend(provenance, snapshot)?;
            serde_json::json!({"snapshot":snapshot,"files":files,"provenance":provenance,"read_policy":policy})
        }
        Command::CheckMission { file } => {
            serde_json::to_value(myr_runner::mission::parse(&read_config(&file)?)?)?
        }
        Command::Seal {
            ir,
            policy,
            mission,
            root,
        } => {
            let ir = serde_json::from_slice(&read_config(&ir)?)?;
            let policy = serde_json::from_slice(&read_config(&policy)?)?;
            let mut graph = open_existing(&root)?;
            let sealed = if let Some(path) = mission {
                let input = myr_runner::mission::parse(&read_config(&path)?)?;
                myr_runner::mission::compile(&mut graph, &input, ir, policy)?
            } else {
                myr_runner::goal::compile(&mut graph, ir, policy)?
            };
            serde_json::json!({"reference":sealed.reference(), "scope":sealed.scope(), "ir":sealed.ir(), "policy":sealed.policy()})
        }
        Command::InspectGoal { cid, root } => {
            let graph = open_existing(&root)?;
            let sealed = myr_runner::goal::load(&graph, graph.resolve(cid)?)?;
            serde_json::json!({"reference":sealed.reference(), "scope":sealed.scope(), "ir":sealed.ir(), "policy":sealed.policy()})
        }
        Command::Pilot { output } => serde_json::to_value(myr_runner::pilot::run(&output)?)?,
        Command::Show { cid, root } => {
            let graph = open_existing(&root)?;
            let reference = graph.resolve(cid)?;
            let live = graph.live(reference)?;
            if matches!(reference.kind, Kind::Artifact | Kind::Goal) {
                serde_json::json!({"reference":reference,"live":live,"content_base64":STANDARD.encode(graph.cas().get(reference)?)})
            } else {
                serde_json::json!({"reference":reference,"live":live,"object":graph.get(reference)?})
            }
        }
        Command::Invalidate { cid, root, reason } => {
            if reason.trim().is_empty() {
                return Err("invalidation reason must not be empty".into());
            }
            let mut graph = open_existing(&root)?;
            let reference = graph.resolve(cid)?;
            reference.require(Kind::Assumption)?;
            if !graph.live(reference)? {
                return Err("assumption is already inactive or stale".into());
            }
            let Object::Assumption(mut assumption) = graph.get(reference)? else {
                unreachable!()
            };
            assumption.invalidates = Some(reference);
            assumption.rationale = graph.register_artifact(reason.as_bytes())?;
            let event = graph.insert(Authority::Runtime, &Object::Assumption(assumption))?;
            serde_json::json!({"invalidated":reference,"event":event,"live":graph.live(reference)?})
        }
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    if mission_failed {
        return Err(
            "mission did not complete; see result.json and the recorded failure references".into(),
        );
    }
    Ok(())
}

fn create_run_store(root: &Path) -> Result<Graph, Box<dyn std::error::Error>> {
    if let Some(parent) = root.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(root)?;
    Ok(Graph::open(
        root.join("graph.sqlite"),
        myr_cas::Store::open(root.join("objects"))?,
    )?)
}

fn read_config(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use std::io::Read;
    const LIMIT: u64 = 16 * 1024 * 1024;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err("configuration exceeds 16 MiB limit".into());
    }
    Ok(bytes)
}

fn main() -> std::process::ExitCode {
    match execute(Args::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("myr: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
