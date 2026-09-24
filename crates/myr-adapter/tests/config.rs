use myr_adapter::config::{Backend, ProviderConfig, validate_reviewers};
use myr_core::Lineage;

#[test]
fn billing_choice_and_lineage_are_explicit_and_fail_closed() {
    let openai = ProviderConfig {
        backend: Backend::CodexCli {
            executable: "codex".into(),
        },
        model: "explicit-checkpoint".into(),
        lineage: Lineage {
            provider_id: "openai".into(),
            family_id: "gpt".into(),
            checkpoint_id: "explicit-checkpoint".into(),
            procedure_id: "review-v0".into(),
        },
    };
    let anthropic = ProviderConfig {
        backend: Backend::AnthropicApi {
            api_key_env: "ANTHROPIC_API_KEY".into(),
        },
        model: "explicit-checkpoint".into(),
        lineage: Lineage {
            provider_id: "anthropic".into(),
            family_id: "claude".into(),
            checkpoint_id: "explicit-checkpoint".into(),
            procedure_id: "review-v0".into(),
        },
    };
    validate_reviewers(&openai, &anthropic).unwrap();
    assert!(openai.subscription());
    assert!(!anthropic.subscription());
    assert!(validate_reviewers(&openai, &openai).is_err());
    let mut invalid = anthropic;
    invalid.lineage.provider_id = "gateway".into();
    assert!(invalid.validate().is_err());
    assert!(serde_json::from_str::<Backend>(r#"{"kind":"auto"}"#).is_err());
    assert!(
        serde_json::from_str::<Backend>(
            r#"{"kind":"codex-cli","executable":"codex","fallback":"openai-api"}"#
        )
        .is_err()
    );
}

#[test]
fn fixed_role_configuration_requires_two_independent_reviewers() {
    use myr_adapter::config::{RoleProviders, RoleSlot};
    let mut providers: RoleProviders =
        serde_json::from_str(include_str!("../../../fixtures/providers-test-only.json")).unwrap();
    providers.validate().unwrap();
    assert_eq!(providers.get(RoleSlot::Worker), &providers.worker);
    assert_eq!(RoleSlot::ReviewerB.role(), myr_adapter::Role::Reviewer);
    providers.reviewer_b = providers.reviewer_a.clone();
    assert!(providers.validate().is_err());
}
