//! Fresh candidate files for verifier execution. This is not an OS sandbox.
use crate::{
    Result, delta, goal,
    views::{self, Tree},
};
use myr_core::{Cid, Kind, ObjectRef, Scope, invalid, validate_relative_path};
use myr_graph::Graph;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

/// Owns candidate files until the verifier has finished. The input manifest is
/// kept outside the directory so it cannot become an unintended test input.
pub struct Candidate {
    directory: tempfile::TempDir,
    input_hashes: BTreeMap<String, Cid>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub scope: Scope,
    pub deltas: Vec<ObjectRef>,
    pub writable: Vec<String>,
    pub files: BTreeMap<String, ObjectRef>,
}

/// Runtime-created input record and the files whose bytes it identifies.
/// This records preparation, not execution or a sandbox attestation.
pub struct RecordedCandidate {
    files: Candidate,
    reference: ObjectRef,
}

impl RecordedCandidate {
    pub fn files(&self) -> &Candidate {
        &self.files
    }
    pub fn reference(&self) -> ObjectRef {
        self.reference
    }
}

/// Materialize and atomically register provenance. No temporary path is stored
/// in the manifest, so identical reconstruction inputs have identical identity.
pub fn prepare_recorded(
    graph: &mut Graph,
    sealed_goal: ObjectRef,
    deltas: &[ObjectRef],
    writable: &[String],
) -> Result<RecordedCandidate> {
    let sealed = goal::load(graph, sealed_goal)?;
    let mut deltas = deltas.to_vec();
    deltas.sort();
    if deltas.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(invalid("duplicate candidate DELTA").into());
    }
    let mut writable: Vec<String> = writable
        .iter()
        .map(|p| p.trim_end_matches('/').to_owned())
        .collect();
    for path in &writable {
        validate_relative_path(path)?;
    }
    writable.sort();
    writable.dedup();
    let tree = delta::reconstruct_live(
        graph,
        &sealed.scope(),
        &deltas,
        &writable,
        &sealed.policy().protected,
    )?;
    let files = Candidate::from_tree(&tree)?;
    let manifest = Manifest {
        format: "myr-candidate-v0".into(),
        scope: sealed.scope(),
        deltas,
        writable,
        files: files
            .input_hashes()
            .iter()
            .map(|(p, cid)| (p.clone(), ObjectRef::new(Kind::Artifact, *cid)))
            .collect(),
    };
    let dependencies: Vec<_> = [manifest.scope.goal, manifest.scope.snapshot]
        .into_iter()
        .chain(manifest.deltas.iter().copied())
        .chain(manifest.files.values().copied())
        .collect();
    let reference = graph
        .register_artifact_with_dependencies(&serde_json::to_vec(&manifest)?, &dependencies)?;
    Ok(RecordedCandidate { files, reference })
}

/// Recompute a manifest against current sealed policy and live CAS dependencies.
/// Merely parsing a model-supplied artifact does not establish this provenance.
pub fn load_manifest(graph: &Graph, reference: ObjectRef) -> Result<Manifest> {
    reference.require(Kind::Artifact)?;
    if !graph.live(reference)? {
        return Err(invalid("candidate manifest is inactive").into());
    }
    let bytes = graph.cas().get(reference)?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    if manifest.format != "myr-candidate-v0"
        || serde_json::to_vec(&manifest)? != bytes
        || manifest.deltas.windows(2).any(|p| p[0] >= p[1])
        || manifest.writable.windows(2).any(|p| p[0] >= p[1])
    {
        return Err(invalid("candidate manifest is not canonical").into());
    }
    for path in &manifest.writable {
        validate_relative_path(path)?;
    }
    let sealed = goal::load(graph, manifest.scope.goal)?;
    if manifest.scope != sealed.scope() {
        return Err(invalid("candidate scope differs from seal").into());
    }
    let tree = delta::reconstruct_live(
        graph,
        &manifest.scope,
        &manifest.deltas,
        &manifest.writable,
        &sealed.policy().protected,
    )?;
    let expected: BTreeMap<_, _> = views::hashes(&tree)
        .into_iter()
        .map(|(p, cid)| (p, ObjectRef::new(Kind::Artifact, cid)))
        .collect();
    if manifest.files != expected {
        return Err(invalid("candidate files differ from reconstruction").into());
    }
    let dependencies = graph.dependencies(reference)?;
    for dependency in [manifest.scope.goal, manifest.scope.snapshot]
        .into_iter()
        .chain(manifest.deltas.iter().copied())
        .chain(manifest.files.values().copied())
    {
        if !dependencies.contains(&dependency) || !graph.live(dependency)? {
            return Err(invalid("candidate provenance is absent or inactive").into());
        }
    }
    Ok(manifest)
}

impl Candidate {
    /// Materialize validated opaque bytes in a new directory. Never accepts a
    /// destination checkout and never copies permissions, links, or metadata.
    pub fn from_tree(tree: &Tree) -> Result<Self> {
        views::validate_tree(tree)?;
        let directory = tempfile::Builder::new()
            .prefix("myr-candidate-")
            .tempdir()?;
        for (relative, bytes) in tree {
            let path = directory.path().join(relative);
            fs::create_dir_all(path.parent().expect("candidate file has parent"))?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            file.write_all(bytes)?;
            file.flush()?;
            drop(file);
            if fs::read(&path)? != *bytes {
                return Err(
                    invalid("materialized candidate differs from reconstructed bytes").into(),
                );
            }
        }
        Ok(Self {
            directory,
            input_hashes: views::hashes(tree),
        })
    }

    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    /// Hashes of inputs at creation, not a claim about files after a command.
    pub fn input_hashes(&self) -> &BTreeMap<String, Cid> {
        &self.input_hashes
    }

    /// Explicit cleanup reports errors that implicit drop cannot report.
    pub fn close(self) -> Result<()> {
        self.directory.close()?;
        Ok(())
    }
}

/// Export a live recorded candidate to a new directory. This proves byte
/// identity with the manifest, not mission completion. A failed write may leave
/// a partial directory; existing destinations are never merged or overwritten.
pub fn export(graph: &Graph, reference: ObjectRef, destination: &Path) -> Result<Manifest> {
    let manifest = load_manifest(graph, reference)?;
    let mut tree = Tree::new();
    for (path, artifact) in &manifest.files {
        tree.insert(path.clone(), graph.cas().get(*artifact)?);
    }
    views::validate_tree(&tree)?;
    let name = destination
        .file_name()
        .ok_or_else(|| invalid("export requires a new directory name"))?;
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent)?;
    if parent.starts_with(graph.cas().root()) {
        return Err(invalid("cannot export into the content-addressed store").into());
    }
    let destination = parent.join(name);
    // create_dir is exclusive even for an existing empty directory or symlink.
    fs::create_dir(&destination)?;
    for (relative, bytes) in &tree {
        let path = destination.join(relative);
        fs::create_dir_all(path.parent().expect("validated file has a parent"))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if fs::read(&path)? != *bytes {
            return Err(invalid("exported candidate differs from stored bytes").into());
        }
    }
    // An assumption invalidated during copying must not produce a success.
    if load_manifest(graph, reference)? != manifest {
        return Err(invalid("candidate changed during export").into());
    }
    Ok(manifest)
}

/// Reopen the compiler seal and reconstruct only live dependencies before
/// creating files. Writable paths are supplied by the trusted task runtime.
/// Dependencies still need rechecking before evidence or a verdict is issued.
pub fn prepare(
    graph: &Graph,
    sealed_goal: ObjectRef,
    deltas: &[ObjectRef],
    writable: &[String],
) -> Result<Candidate> {
    let sealed = goal::load(graph, sealed_goal)?;
    let tree = delta::reconstruct_live(
        graph,
        &sealed.scope(),
        deltas,
        writable,
        &sealed.policy().protected,
    )?;
    Candidate::from_tree(&tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_checks_manifest_and_never_overwrites_existing_destinations() {
        let mut f = crate::command_verification::tests::Fixture::new(true);
        let candidate = prepare_recorded(&mut f.graph, f.goal, &[], &[]).unwrap();
        let destination = f._dir.path().join("exported");
        let manifest = export(&f.graph, candidate.reference(), &destination).unwrap();
        for (path, reference) in &manifest.files {
            assert_eq!(
                fs::read(destination.join(path)).unwrap(),
                f.graph.cas().get(*reference).unwrap()
            );
        }
        fs::write(destination.join("sentinel"), b"keep").unwrap();
        assert!(export(&f.graph, candidate.reference(), &destination).is_err());
        assert_eq!(fs::read(destination.join("sentinel")).unwrap(), b"keep");
        let empty = f._dir.path().join("empty");
        fs::create_dir(&empty).unwrap();
        assert!(export(&f.graph, candidate.reference(), &empty).is_err());
        let invalid = f
            .graph
            .register_artifact(b"not a candidate manifest")
            .unwrap();
        let absent = f._dir.path().join("invalid-output");
        assert!(export(&f.graph, invalid, &absent).is_err());
        assert!(!absent.exists());
        let forbidden = f.graph.cas().root().join("exported");
        assert!(export(&f.graph, candidate.reference(), &forbidden).is_err());
        assert!(!forbidden.exists());
    }

    #[test]
    fn opaque_inputs_are_separate_and_cleanup_is_owned() {
        let tree = Tree::from([
            ("nested/raw.bin".into(), vec![0, 255, 13, 10]),
            ("empty".into(), vec![]),
        ]);
        let candidate = Candidate::from_tree(&tree).unwrap();
        let other = Candidate::from_tree(&tree).unwrap();
        assert_ne!(candidate.path(), other.path());
        assert_eq!(candidate.input_hashes(), &views::hashes(&tree));
        for (path, bytes) in &tree {
            assert_eq!(&fs::read(candidate.path().join(path)).unwrap(), bytes);
        }
        fs::write(candidate.path().join("nested/raw.bin"), b"test output").unwrap();
        assert_eq!(
            fs::read(other.path().join("nested/raw.bin")).unwrap(),
            tree["nested/raw.bin"]
        );
        let path = candidate.path().to_owned();
        candidate.close().unwrap();
        assert!(!path.exists());
        let path = other.path().to_owned();
        drop(other);
        assert!(!path.exists());
    }

    #[test]
    fn unsafe_and_platform_ambiguous_trees_are_rejected() {
        for paths in [
            vec!["../escape"],
            vec!["CON"],
            vec!["a", "a/b"],
            vec!["A/x", "a/y"],
            vec!["a", "A"],
        ] {
            let tree = paths.into_iter().map(|p| (p.into(), vec![1])).collect();
            assert!(Candidate::from_tree(&tree).is_err());
        }
    }
}
