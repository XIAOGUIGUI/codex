use super::*;
use crate::config::test_config;
use codex_models_manager::model_info::model_info_from_slug;
use codex_protocol::config_types::AdaptiveAutoCompactConfig;
use codex_protocol::config_types::ContextManagementConfig;
use codex_protocol::config_types::ModelContextProfile;
use pretty_assertions::assert_eq;
use std::collections::HashMap;

#[tokio::test]
async fn adaptive_limit_uses_percentage_for_standard_windows() {
    let mut config = test_config().await;
    config.context_management.auto_compact = Some(AdaptiveAutoCompactConfig::default());
    let mut model = model_info_from_slug("third-party-200k");
    model.context_window = Some(200_000);
    model.max_context_window = Some(200_000);

    assert_eq!(auto_compact_token_limit(&config, &model), Some(150_000));
}

#[tokio::test]
async fn adaptive_limit_caps_large_windows_for_cost_control() {
    let mut config = test_config().await;
    config.context_management.auto_compact = Some(AdaptiveAutoCompactConfig::default());
    let mut model = model_info_from_slug("third-party-1m");
    model.context_window = Some(1_000_000);
    model.max_context_window = Some(1_000_000);

    assert_eq!(auto_compact_token_limit(&config, &model), Some(300_000));
}

#[tokio::test]
async fn exact_model_trigger_takes_precedence() {
    let mut config = test_config().await;
    config.context_management = ContextManagementConfig {
        auto_compact: Some(AdaptiveAutoCompactConfig {
            models: HashMap::from([(
                "third-party-1m".to_string(),
                ModelContextProfile {
                    context_window: Some(1_000_000),
                    trigger_tokens: Some(280_000),
                },
            )]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut model = model_info_from_slug("third-party-1m");
    model.context_window = Some(1_000_000);

    assert_eq!(auto_compact_token_limit(&config, &model), Some(280_000));
}

#[tokio::test]
async fn legacy_model_limit_remains_active_without_adaptive_config() {
    let config = test_config().await;
    let mut model = model_info_from_slug("legacy-model");
    model.context_window = Some(200_000);
    model.auto_compact_token_limit = Some(123_000);

    assert_eq!(auto_compact_token_limit(&config, &model), Some(123_000));
}
