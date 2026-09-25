//! Primary-condition source view. Only the planner's source pass reads it, after
//! its plan is frozen. Its bytes stay in memory: no source-view blob is written
//! to shared CAS or indexed in the graph, and no CID of a private-only file can
//! become a shared reference. The source agent can transfer information only
//! through the pipeline's inter-agent protocol.
use crate::{
    Result,
    views::{Tree, validate_tree},
};
use myr_core::{Kind, ObjectRef};
use myr_graph::Graph;
use std::collections::{BTreeMap, BTreeSet};

pub struct SourceView {
    tree: Tree,
}

/// What one source-pass session receives: the listing and the private bytes.
#[derive(Clone)]
pub struct PrivateView {
    pub listing: BTreeMap<String, ObjectRef>,
    pub files: BTreeMap<ObjectRef, Vec<u8>>,
}

impl SourceView {
    pub fn new(tree: Tree) -> Result<Self> {
        validate_tree(&tree)?;
        if tree.is_empty() {
            return Err(myr_core::invalid("source view is empty").into());
        }
        Ok(Self { tree })
    }

    fn reference(bytes: &[u8]) -> ObjectRef {
        ObjectRef::new(Kind::Artifact, myr_wire::artifact_cid(bytes))
    }

    /// Path listing shown only to the source agent.
    pub fn listing(&self) -> BTreeMap<String, ObjectRef> {
        self.tree
            .iter()
            .map(|(path, bytes)| (path.clone(), Self::reference(bytes)))
            .collect()
    }

    pub fn by_reference(&self) -> BTreeMap<ObjectRef, Vec<u8>> {
        self.tree
            .values()
            .map(|bytes| (Self::reference(bytes), bytes.clone()))
            .collect()
    }

    pub fn private_view(&self) -> PrivateView {
        PrivateView {
            listing: self.listing(),
            files: self.by_reference(),
        }
    }
}

impl PrivateView {
    /// Files whose bytes do not occur in the shared baseline.
    pub fn private_only(&self, baseline: &BTreeSet<ObjectRef>) -> BTreeSet<ObjectRef> {
        self.files
            .keys()
            .filter(|r| !baseline.contains(r))
            .copied()
            .collect()
    }

    /// Post-run check of the CAS rule: no private-only blob reached shared CAS.
    pub fn absent_from_shared(
        &self,
        graph: &Graph,
        baseline: &BTreeSet<ObjectRef>,
    ) -> Result<bool> {
        for reference in self.private_only(baseline) {
            if graph.cas().exists(reference)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Fixed instruction for the source pass in both pipelines.
pub const INSTRUCTION: &str = "Your plan is frozen and you cannot change tasks, capabilities or the goal. You can now read the repository as it currently is. Pass on to the other agents, through the actions available to you, any information from it that they need; then finish.";
