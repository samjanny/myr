use myr_core::{Lineage, ValidationError, invalid};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The transport itself selects billing. There is no `auto` or fallback variant.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Backend {
    CodexCli { executable: PathBuf },
    ClaudeCode { executable: PathBuf },
    OpenaiApi { api_key_env: String },
    AnthropicApi { api_key_env: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub backend: Backend,
    pub model: String,
    pub lineage: Lineage,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoleSlot {
    Planner,
    Worker,
    ReviewerA,
    ReviewerB,
}

impl RoleSlot {
    pub fn role(self) -> crate::Role {
        match self {
            Self::Planner => crate::Role::Planner,
            Self::Worker => crate::Role::Worker,
            Self::ReviewerA | Self::ReviewerB => crate::Role::Reviewer,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleProviders {
    pub planner: ProviderConfig,
    pub worker: ProviderConfig,
    pub reviewer_a: ProviderConfig,
    pub reviewer_b: ProviderConfig,
}

impl RoleProviders {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.planner.validate()?;
        self.worker.validate()?;
        validate_reviewers(&self.reviewer_a, &self.reviewer_b)
    }
    /// Reject every separately billed API backend. Used when the operator has
    /// authorized only existing Claude Code / Codex CLI subscription sessions.
    pub fn require_subscription(&self) -> Result<(), ValidationError> {
        for slot in [
            RoleSlot::Planner,
            RoleSlot::Worker,
            RoleSlot::ReviewerA,
            RoleSlot::ReviewerB,
        ] {
            if !self.get(slot).subscription() {
                return Err(invalid(
                    "subscription-only run rejects separately billed API backends",
                ));
            }
        }
        Ok(())
    }
    pub fn get(&self, slot: RoleSlot) -> &ProviderConfig {
        match slot {
            RoleSlot::Planner => &self.planner,
            RoleSlot::Worker => &self.worker,
            RoleSlot::ReviewerA => &self.reviewer_a,
            RoleSlot::ReviewerB => &self.reviewer_b,
        }
    }
}

impl ProviderConfig {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.model.trim().is_empty() || !self.lineage.known() {
            return Err(invalid(
                "provider requires explicit model and complete lineage",
            ));
        }
        let producer = match &self.backend {
            Backend::CodexCli { executable } | Backend::ClaudeCode { executable } => {
                if executable.as_os_str().is_empty() {
                    return Err(invalid("CLI executable is required"));
                }
                if matches!(self.backend, Backend::CodexCli { .. }) {
                    "openai"
                } else {
                    "anthropic"
                }
            }
            Backend::OpenaiApi { api_key_env } | Backend::AnthropicApi { api_key_env } => {
                if api_key_env.is_empty()
                    || !api_key_env
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    return Err(invalid(
                        "API credentials require an environment variable name, never a literal secret",
                    ));
                }
                if matches!(self.backend, Backend::OpenaiApi { .. }) {
                    "openai"
                } else {
                    "anthropic"
                }
            }
        };
        if self.lineage.provider_id != producer {
            return Err(invalid(
                "configured lineage producer does not match backend",
            ));
        }
        Ok(())
    }
    pub fn subscription(&self) -> bool {
        matches!(
            self.backend,
            Backend::CodexCli { .. } | Backend::ClaudeCode { .. }
        )
    }
}

pub fn validate_reviewers(a: &ProviderConfig, b: &ProviderConfig) -> Result<(), ValidationError> {
    a.validate()?;
    b.validate()?;
    if !a.lineage.different_from(&b.lineage) {
        return Err(invalid(
            "reviewers require different producers and model families",
        ));
    }
    Ok(())
}
