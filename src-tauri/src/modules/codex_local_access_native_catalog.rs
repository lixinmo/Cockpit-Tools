// Public protocol catalogs expose known capabilities without bootstrap guesses.
fn native_public_model_entry(
    collection: &CodexLocalAccessCollection,
    plan: &NativeModelRoutePlan,
    account: &CodexAccount,
    id: &str,
    upstream: &str,
) -> Value {
    let template = native_declared_provider_model_metadata(account, upstream).or_else(|| {
        (native_wire_api(account).ok() == Some(native_contract::WireApi::Responses))
            .then(|| codex_protocol::known_codex_model_template(upstream))
            .flatten()
    });
    let window = account
        .api_model_context_windows
        .get(upstream)
        .copied()
        .filter(|n| *n > 0);
    let vision = native_catalog_vision(collection, plan, account, upstream);
    let tools = match native_capability_support(collection, account, upstream, "function") {
        Some(CodexLocalAccessCapabilitySupport::Supported) => true,
        Some(CodexLocalAccessCapabilitySupport::Unsupported) => false,
        None => !account.is_api_key_auth() || template.is_some(),
    };
    let mut entry = native_model_entry_from_metadata(id, template.as_ref(), window, vision, tools);
    entry["model_info"]["upstream_id"] = json!(upstream);
    entry
}

fn native_model_entry_from_metadata(
    id: &str,
    template: Option<&Value>,
    window: Option<i64>,
    vision: bool,
    tools: bool,
) -> Value {
    let mut entry = json!({"id":id,"object":"model","created":0,"owned_by":"configured-upstream"});
    let mut capabilities = vec!["completion"];
    if tools {
        capabilities.push("tools");
    }
    if vision {
        capabilities.push("vision");
    }
    let levels = template
        .filter(|template| template["cockpit_reasoning_known"] != false)
        .and_then(|template| template["supported_reasoning_levels"].as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().or_else(|| item["effort"].as_str()))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        });
    if levels
        .as_ref()
        .is_some_and(|levels| levels.iter().any(|level| level != "none"))
    {
        capabilities.push("thinking");
    }
    entry["capabilities"] = json!(capabilities);
    entry["model_info"] = json!({"upstream_id":id,"input_modalities":if vision {vec!["text","image"]} else {vec!["text"]}});
    // Missing capabilities are unknown, whereas an explicit empty array means
    // unsupported. Keep that distinction when another gateway discovers us.
    if let Some(levels) = &levels {
        entry["model_info"]["supported_reasoning_efforts"] = json!(levels);
    }
    if let Some(template) = template {
        for field in ["display_name", "description"] {
            if let Some(value) = template.get(field) {
                entry[field] = value.clone();
            }
        }
        if let Some(effort) = template
            .get("default_reasoning_level")
            .filter(|_| levels.is_some())
        {
            entry["model_info"]["default_reasoning_effort"] = effort.clone();
        }
    }
    if let Some(window) = window.or_else(|| {
        template
            .filter(|template| template["cockpit_context_known"] != false)
            .and_then(|template| template["context_window"].as_i64())
            .filter(|n| *n > 0)
    }) {
        entry["context_length"] = json!(window);
        entry["model_info"]["context_length"] = json!(window);
    }
    entry
}

#[cfg(test)]
mod native_catalog_metadata_tests {
    use super::*;
    #[test]
    fn grok_47_native_catalog_keeps_known_capabilities_and_explicit_vision_permission() {
        let template = codex_protocol::known_codex_model_template("grok-4.7").unwrap();
        for vision in [false, true] {
            let model =
                native_model_entry_from_metadata("grok-4.7", Some(&template), None, vision, true);
            assert_eq!(model["context_length"], 500_000);
            assert_eq!(model["model_info"]["default_reasoning_effort"], "high");
            assert_eq!(
                model["model_info"]["supported_reasoning_efforts"],
                json!(["low", "medium", "high", "xhigh"])
            );
            let capabilities = model["capabilities"].as_array().unwrap();
            assert!(capabilities.contains(&json!("thinking")));
            assert!(capabilities.contains(&json!("tools")));
            assert_eq!(capabilities.contains(&json!("vision")), vision);
        }
    }
    #[test]
    fn known_capabilities_and_explicit_window_reach_ollama_and_gemini() {
        let template = json!({"display_name":"Known","context_window":9000,"supported_reasoning_levels":[{"effort":"high"}],"default_reasoning_level":"high"});
        let model =
            native_model_entry_from_metadata("known", Some(&template), Some(7000), true, true);
        assert_eq!(model["context_length"], 7000);
        assert_eq!(model["created"], 0);
        let shown = native_api::compat_catalog_show_for_test(&model).unwrap();
        assert!(shown["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("tools")));
        assert!(shown["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("thinking")));
        assert_eq!(shown["model_info"]["context_length"], 7000);
    }
    #[test]
    fn unknown_provider_does_not_advertise_bootstrap_context_or_thinking() {
        let entry = native_model_entry_from_metadata("custom", None, None, false, false);
        assert!(entry.get("context_length").is_none());
        assert_eq!(entry["capabilities"], json!(["completion"]));
        assert!(entry["model_info"]
            .get("supported_reasoning_efforts")
            .is_none());
        assert!(entry["model_info"]
            .get("default_reasoning_effort")
            .is_none());
    }

    #[test]
    fn provider_metadata_roundtrip_keeps_unknown_distinct_from_explicit_unsupported() {
        let mut fallback = codex_protocol::build_provider_client_model("custom", 0);
        fallback["cockpit_context_known"] = json!(false);
        for template in [None, Some(&fallback)] {
            let entry = native_model_entry_from_metadata("custom", template, None, false, false);
            assert!(entry["model_info"]
                .get("supported_reasoning_efforts")
                .is_none());
            assert!(entry["model_info"]
                .get("default_reasoning_effort")
                .is_none());
            let parsed = crate::modules::codex_provider_metadata::parse_metadata(&entry).unwrap();
            assert_eq!(parsed.reasoning_efforts, None);
            assert_eq!(parsed.default_reasoning_effort, None);
            assert_eq!(parsed.context_window, None);
            let chained = crate::modules::codex_provider_metadata::parse_models(
                &json!({"data":[entry], "models":[fallback.clone()]}),
            )
            .unwrap();
            assert_eq!(
                chained[0].metadata.as_ref().unwrap().reasoning_efforts,
                None
            );
        }
        let unsupported = json!({"supported_reasoning_levels":[],"default_reasoning_level":null});
        let entry =
            native_model_entry_from_metadata("no-effort", Some(&unsupported), None, false, false);
        assert_eq!(
            entry["model_info"]["supported_reasoning_efforts"],
            json!([])
        );
        assert_eq!(
            crate::modules::codex_provider_metadata::parse_metadata(&entry)
                .unwrap()
                .reasoning_efforts,
            Some(vec![])
        );
    }
}
