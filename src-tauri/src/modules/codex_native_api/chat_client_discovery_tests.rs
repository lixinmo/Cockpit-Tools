use super::*;

fn search() -> Value {
    json!({"type":"tool_search","execution":"client","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}})
}

fn request(tools: Value, input: Value) -> Value {
    json!({"model":"fixture","tools":tools,"input":input})
}

fn wire(body: &Value) -> String {
    responses_to_chat(body).unwrap()["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|tool| {
            tool["function"]["name"]
                .as_str()
                .filter(|name| name.starts_with("cockpit_client_tool_search"))
        })
        .unwrap()
        .into()
}

fn completion(name: &str, arguments: &str) -> Value {
    json!({"id":"reply","model":"fixture","choices":[{"index":0,"finish_reason":"tool_calls",
        "message":{"role":"assistant","tool_calls":[{"id":"search-call","type":"function","function":{"name":name,"arguments":arguments}}]}}]})
}

fn history(tools: Value) -> Value {
    json!([
        {"type":"tool_search_call","execution":"client","call_id":"search-call","arguments":{"query":"find tools"}},
        {"type":"tool_search_output","call_id":"search-call","tools":tools}
    ])
}

#[test]
fn chat_client_discovery_json_roundtrip_preserves_scope_custom_and_namespaces() {
    let body = request(
        json!([search(),
            {"type":"function","name":"read","parameters":{"type":"object"}},
            {"type":"namespace","name":"files","tools":[{"type":"custom","name":"patch","format":{"type":"text"}}]}
        ]),
        json!([]),
    );
    let original = body.clone();
    let (_, context) = ToolContext::prepare_responses(&body).unwrap();
    let upstream = responses_to_chat(&body).unwrap();
    assert_eq!(body, original);
    assert!(upstream["tools"]
        .as_array()
        .unwrap()
        .iter()
        .all(|tool| tool["type"] == "function"));
    let private = wire(&body);
    let found = upstream["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["function"]["name"] == private)
        .unwrap();
    assert_eq!(found["function"]["parameters"], search()["parameters"]);
    let response = chat_to_response_with_context(
        &completion(&private, "{\"query\":\"查找\"}"),
        "fixture",
        &context,
    )
    .unwrap();
    assert_eq!(response["output"][0]["type"], "tool_search_call");
    assert_eq!(response["output"][0]["execution"], "client");
    assert_eq!(response["output"][0]["arguments"], json!({"query":"查找"}));
    assert_eq!(response["output"][0]["call_id"], "search-call");
    assert!(!response.to_string().contains(&private));
    let patched = chat_to_response_with_context(
        &completion(
            "files__patch",
            "{\"input\":\"*** Begin Patch\\n*** End Patch\"}",
        ),
        "fixture",
        &context,
    )
    .unwrap();
    assert_eq!(patched["output"][0]["type"], "custom_tool_call");
    assert_eq!(patched["output"][0]["name"], "patch");
    assert_eq!(patched["output"][0]["namespace"], "files");
    assert_eq!(
        patched["output"][0]["input"],
        "*** Begin Patch\n*** End Patch"
    );
}

#[test]
fn chat_client_discovery_schema_precedence_and_explicit_empty_tools_survive_history() {
    let old = json!({"type":"function","namespace":"files","name":"read","parameters":{"type":"object","old":true}});
    let new = json!({"type":"function","namespace":"files","name":"read","parameters":{"type":"object","new":true}});
    let current = json!({"type":"function","namespace":"files","name":"read","parameters":{"type":"object","current":true}});
    let mut input = history(json!([old]));
    input.as_array_mut().unwrap().extend(json!([
        {"type":"tool_search_call","execution":"client","call_id":"next","arguments":{"query":"refresh"}},
        {"type":"tool_search_output","execution":"client","call_id":"next","output":{"tools":[new]}}
    ]).as_array().unwrap().clone());
    for (declarations, marker) in [
        (json!([search()]), "new"),
        (json!([search(), current]), "current"),
    ] {
        let body = request(declarations, input.clone());
        let chat = responses_to_chat(&body).unwrap();
        let found = chat["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["function"]["name"] == "files__read")
            .unwrap();
        assert_eq!(found["function"]["parameters"][marker], true);
        assert_eq!(chat["messages"][0]["tool_calls"][0]["id"], "search-call");
        assert_eq!(chat["messages"][1]["tool_call_id"], "search-call");
    }
    let disabled = responses_to_chat(&request(json!([]), input)).unwrap();
    assert!(disabled.get("tools").is_none());
    assert_eq!(disabled["messages"].as_array().unwrap().len(), 4);
}

#[test]
fn chat_client_discovery_only_explicit_client_tools_and_paired_history_are_admitted() {
    for tool in [
        json!({"type":"tool_search"}),
        json!({"type":"tool_search","execution":"server"}),
        json!({"type":"web_search"}),
        json!({"type":"mcp"}),
        json!({"type":"file_search"}),
        json!({"type":"function","name":"f","allowed_callers":["programmatic"]}),
    ] {
        assert!(responses_to_chat(&request(json!([tool.clone()]), json!([]))).is_err());
        assert!(
            responses_to_chat(&request(json!([search()]), history(json!([tool.clone()])))).is_err()
        );
        let mut input = history(json!([]));
        input[1].as_object_mut().unwrap().remove("tools");
        input[1]["output"] = json!(json!({"tools":[tool]}).to_string());
        assert!(responses_to_chat(&request(json!([search()]), input)).is_err());
    }
    for input in [
        json!([{"type":"tool_search_output","execution":"client","call_id":"orphan","tools":[]}]),
        json!([{"type":"tool_search_call","execution":"client","call_id":"pending","arguments":{}}]),
        json!([{"type":"tool_search_call","execution":"client","call_id":"same","arguments":{}},{"type":"tool_search_call","execution":"client","call_id":"same","arguments":{}},{"type":"tool_search_output","call_id":"same","tools":[]}]),
        json!([{"type":"tool_search_call","execution":"server","call_id":"server","arguments":{}},{"type":"tool_search_output","call_id":"server","tools":[]}]),
    ] {
        assert!(responses_to_chat(&request(json!([search()]), input)).is_err());
    }
    let ambiguous = json!({"input":[{"type":"tool_search_call","call_id":"ambiguous","arguments":{}},{"type":"tool_search_output","call_id":"ambiguous","tools":[]}]});
    assert!(responses_to_chat(&ambiguous).is_err());
}

#[test]
fn chat_client_discovery_choice_requires_the_explicit_current_declaration() {
    for tools in [
        json!([]),
        json!([{"type":"function","name":"read","parameters":{"type":"object"}}]),
    ] {
        let mut body = request(tools, history(json!([])));
        body["tool_choice"] = json!({"type":"tool_search"});
        assert!(responses_to_chat(&body).is_err());
    }
    let mut body = request(json!([search()]), json!([]));
    body["tool_choice"] =
        json!({"type":"allowed_tools","mode":"required","tools":[{"type":"tool_search"}]});
    let chat = responses_to_chat(&body).unwrap();
    assert_eq!(
        chat["tool_choice"]["allowed_tools"]["tools"][0]["function"]["name"],
        wire(&body)
    );
    body["tool_choice"] = json!({"type":"tool_search","execution":"server"});
    assert!(responses_to_chat(&body).is_err());
}

#[test]
fn chat_client_discovery_private_names_do_not_shadow_functions_or_other_requests() {
    let body = request(
        json!([search(), {"type":"function","name":"cockpit_client_tool_search","parameters":{"type":"object"}}]),
        json!([]),
    );
    let (_, context) = ToolContext::prepare_responses(&body).unwrap();
    let chat = responses_to_chat(&body).unwrap();
    let hidden = chat["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|tool| {
            tool["function"]["name"]
                .as_str()
                .filter(|name| name.starts_with("cockpit_client_tool_search_"))
        })
        .unwrap();
    let response = chat_to_response_with_context(
        &completion("cockpit_client_tool_search", "{}"),
        "fixture",
        &context,
    )
    .unwrap();
    assert_eq!(response["output"][0]["type"], "function_call");
    let (_, isolated) = ToolContext::prepare_responses(&request(json!([]), json!([]))).unwrap();
    let response =
        chat_to_response_with_context(&completion(hidden, "{}"), "fixture", &isolated).unwrap();
    assert_eq!(response["output"][0]["type"], "function_call");
}

#[test]
fn chat_client_discovery_nested_function_shape_preserves_names_and_collision_fencing() {
    let ordinary = json!({"type":"function","function":{"name":"cockpit_client_tool_search","parameters":{"type":"object","ordinary":true}}});
    let body = request(
        json!([search(), ordinary, {"type":"namespace","name":"files","tools":[
            {"type":"function","function":{"name":"read","parameters":{"type":"object","read":true}}},
            {"type":"function","function":{"name":"write","parameters":{"type":"object","write":true}}}
        ]}]),
        json!([]),
    );
    let chat = responses_to_chat(&body).unwrap();
    let tools = chat["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 4);
    let names = tools
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(names.contains(&"cockpit_client_tool_search"));
    assert!(names.contains(&"cockpit_client_tool_search_1"));
    assert!(names.contains(&"files__read") && names.contains(&"files__write"));
    let (_, context) = ToolContext::prepare_responses(&body).unwrap();
    let response = chat_to_response_with_context(
        &completion("cockpit_client_tool_search", "{}"),
        "fixture",
        &context,
    )
    .unwrap();
    assert_eq!(response["output"][0]["type"], "function_call");
    let mut ordinary_only = body;
    ordinary_only["tools"].as_array_mut().unwrap().remove(0);
    assert_eq!(
        responses_to_chat(&ordinary_only).unwrap()["tools"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn chat_client_discovery_sse_restores_unicode_and_terminal_once_for_eof_and_done() {
    let body = request(json!([search()]), json!([]));
    let hidden = wire(&body);
    for done in [false, true] {
        let (_, context) = ToolContext::prepare_responses(&body).unwrap();
        let mut adapter = ResponsesStreamAdapter::with_context("fixture".into(), context);
        let mut events = adapter.push(&json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"search-call","type":"function","function":{"name":hidden,"arguments":"{\"query\":\"查"}}]},"finish_reason":null}]})).unwrap();
        events.extend(adapter.push(&json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"找工具\"}"}}]},"finish_reason":"tool_calls"}]})).unwrap());
        events.extend(adapter.push(&json!({"choices":[],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}})).unwrap());
        events.extend(if done {
            adapter.finish_done().unwrap()
        } else {
            adapter.finish().unwrap()
        });
        assert!(events.iter().all(|event| event["type"]
            != "response.function_call_arguments.delta"
            && event["type"] != "response.function_call_arguments.done"));
        let item = &events
            .iter()
            .find(|event| event["type"] == "response.output_item.done")
            .unwrap()["item"];
        assert_eq!(item["type"], "tool_search_call");
        assert_eq!(item["call_id"], "search-call");
        assert_eq!(item["arguments"], json!({"query":"查找工具"}));
        assert_eq!(events.last().unwrap()["response"]["output"][0], *item);
        assert_eq!(
            events.last().unwrap()["response"]["usage"]["total_tokens"],
            5
        );
        assert!(!serde_json::to_string(&events).unwrap().contains(&hidden));
        assert!(adapter.finish_done().unwrap().is_empty());
    }
}

#[test]
fn chat_client_discovery_invalid_search_arguments_never_seal_a_success() {
    let body = request(json!([search()]), json!([]));
    let hidden = wire(&body);
    for arguments in ["[]", "null", "1", "{\"query\":"] {
        let (_, context) = ToolContext::prepare_responses(&body).unwrap();
        assert!(chat_to_response_with_context(
            &completion(&hidden, arguments),
            "fixture",
            &context
        )
        .is_err());
        let mut adapter = ResponsesStreamAdapter::with_context("fixture".into(), context);
        let events = adapter.push(&json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"search-call","type":"function","function":{"name":hidden,"arguments":arguments}}]},"finish_reason":"tool_calls"}]})).unwrap();
        assert!(events
            .iter()
            .all(|event| event["type"] != "response.output_item.done"));
        assert!(adapter.finish_done().is_err());
    }
}
