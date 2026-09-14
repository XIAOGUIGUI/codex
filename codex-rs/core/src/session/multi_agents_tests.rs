use super::multi_agents::effective_multi_agent_mode;
use super::tests::make_session_and_context;
use codex_model_provider::create_model_provider;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::config_types::MultiAgentMode;
use codex_protocol::protocol::MultiAgentVersion;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn custom_providers_default_to_proactive_delegation() {
    let (_session, mut turn) = make_session_and_context().await;
    turn.multi_agent_version = MultiAgentVersion::V2;
    turn.provider = create_model_provider(
        ModelProviderInfo {
            name: "custom".to_string(),
            base_url: Some("https://example.test/v1".to_string()),
            ..ModelProviderInfo::default()
        },
        /*auth_manager*/ None,
    );
    assert_eq!(
        effective_multi_agent_mode(&turn),
        Some(MultiAgentMode::Proactive)
    );
}
