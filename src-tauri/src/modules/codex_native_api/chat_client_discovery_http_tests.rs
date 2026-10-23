//! Client discovery and tool-result replay through a real Chat loopback server.
use super::*;

#[derive(Clone, Copy, Debug)]
enum Delivery {
    Json,
    Sse,
    WebSocket,
}

#[derive(Clone, Copy, Debug)]
enum Upstream {
    Json,
    SseDone,
    SseEof,
}

fn search() -> Value {
    json!({"type":"tool_search","execution":"client","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}})
}

fn discovered() -> Value {
    json!([
        {"type":"namespace","name":"workspace","tools":[{"type":"function","name":"read_file","defer_loading":true,"strict":true,"parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}}]},
        {"type":"function","name":"observe","defer_loading":true,"parameters":{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}}
    ])
}

fn search_name(body: &Value) -> String {
    body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|tool| {
            tool.pointer("/function/name")
                .and_then(Value::as_str)
                .filter(|name| name.starts_with("cockpit_client_tool_search"))
        })
        .expect("Chat must carry the lowered client search declaration")
        .to_owned()
}

fn chat_reply(round: usize, body: &Value, wire: Upstream, bad_args: Option<&str>) -> Response {
    let calls = if round == 1 {
        vec![
            json!({"id":"call_search","type":"function","function":{"name":search_name(body),"arguments":bad_args.unwrap_or("{\"query\":\"查找文件\"}")}}),
        ]
    } else if round == 2 {
        vec![
            json!({"id":"call_read","type":"function","function":{"name":"workspace__read_file","arguments":"{\"path\":\"fixture.txt\"}"}}),
            json!({"id":"call_observe","type":"function","function":{"name":"observe","arguments":"{\"value\":\"fixture\"}"}}),
        ]
    } else {
        vec![]
    };
    let reason = if calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    let mut message = json!({"role":"assistant","content":null});
    if calls.is_empty() {
        message["content"] = json!("Fixture tool results received.");
    } else {
        message["tool_calls"] = json!(calls);
    }
    if matches!(wire, Upstream::Json) {
        return Response::builder()
            .header("content-type", "application/json")
            .body(Body::from(json!({"id":format!("chat_{round}"),"model":"m","choices":[{"index":0,"message":message,"finish_reason":reason}],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}).to_string()))
            .unwrap();
    }
    let mut frames = Vec::new();
    if calls.is_empty() {
        frames.push(json!({"id":format!("chat_{round}"),"choices":[{"index":0,"delta":{"content":"Fixture tool results received."},"finish_reason":"stop"}]}));
    } else {
        for (index, call) in calls.iter().enumerate() {
            let args = call["function"]["arguments"].as_str().unwrap();
            let split = args
                .char_indices()
                .map(|(index, _)| index)
                .find(|index| *index >= args.len() / 2)
                .unwrap_or(args.len());
            frames.push(json!({"id":format!("chat_{round}"),"choices":[{"index":0,"delta":{"tool_calls":[{"index":index,"id":call["id"],"type":"function","function":{"name":call["function"]["name"],"arguments":&args[..split]}}]},"finish_reason":null}]}));
            frames.push(json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":index,"function":{"arguments":&args[split..]}}]},"finish_reason":null}]}));
        }
        frames.push(json!({"choices":[{"index":0,"delta":{},"finish_reason":reason}]}));
    }
    // Usage may arrive after finish_reason, before either EOF or [DONE].
    frames.push(
        json!({"choices":[],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}),
    );
    let mut payload: String = frames
        .iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect();
    if matches!(wire, Upstream::SseDone) {
        payload.push_str("data: [DONE]\n\n");
    }
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from(payload))
        .unwrap()
}

async fn exchange(
    handle: &native::Handle,
    ws: &mut Option<Socket>,
    mut body: Value,
    delivery: Delivery,
) -> (Option<StatusCode>, Vec<Value>, String) {
    if matches!(delivery, Delivery::WebSocket) {
        let ws = ws.as_mut().unwrap();
        body["type"] = json!("response.create");
        send(ws, body).await;
        // Retain every event so private function argument events cannot hide
        // behind a correctly restored terminal response.
        return tokio::time::timeout(Duration::from_secs(5), async {
            let mut events = Vec::new();
            let mut raw = String::new();
            loop {
                match ws.next().await.unwrap().unwrap() {
                    Message::Text(text) => {
                        let event: Value = serde_json::from_str(&text).unwrap();
                        let terminal = matches!(
                            event["type"].as_str(),
                            Some(
                                "response.completed"
                                    | "response.done"
                                    | "response.failed"
                                    | "response.incomplete"
                                    | "response.cancelled"
                                    | "error"
                            )
                        );
                        raw.push_str(&text);
                        events.push(event);
                        if terminal {
                            return (None, events, raw);
                        }
                    }
                    Message::Ping(value) => ws.send(Message::Pong(value)).await.unwrap(),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
    }
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
        .post(format!("http://{}/v1/responses", handle.addr))
        .bearer_auth("key")
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let json_response = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("application/json"));
    let raw = response.text().await.expect("readable gateway response");
    let events = if matches!(delivery, Delivery::Json) || json_response || !status.is_success() {
        vec![serde_json::from_str(&raw).unwrap()]
    } else {
        raw.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    };
    (Some(status), events, raw)
}

fn completed(status: Option<StatusCode>, events: &[Value], delivery: Delivery) -> Value {
    assert!(status.is_none_or(|status| status.is_success()));
    // An upstream JSON response is also returned as JSON to an HTTP caller
    // requesting a stream. WebSocket delivery expands JSON into item events.
    if matches!(delivery, Delivery::Json)
        || events
            .first()
            .is_some_and(|event| event["object"] == "response")
    {
        assert_eq!(events[0]["status"], "completed");
        return events[0].clone();
    }
    let terminal = events.last().unwrap();
    assert_eq!(terminal["type"], "response.completed", "{terminal}");
    let response = terminal["response"].clone();
    let done: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "response.output_item.done")
        .map(|event| event["item"].clone())
        .collect();
    assert_eq!(response["output"], json!(done));
    response
}

#[tokio::test]
async fn chat_client_discovery_completes_three_rounds_over_json_sse_and_websocket() {
    for delivery in [Delivery::Json, Delivery::Sse, Delivery::WebSocket] {
        for wire in [Upstream::Json, Upstream::SseDone, Upstream::SseEof] {
            let captures = Arc::new(Mutex::new(Vec::<Value>::new()));
            let seen = captures.clone();
            let app = Router::new().route(
                "/v1/chat/completions",
                post(move |Json(body): Json<Value>| {
                    let seen = seen.clone();
                    async move {
                        let round = {
                            let mut captures = seen.lock().unwrap();
                            captures.push(body.clone());
                            captures.len()
                        };
                        chat_reply(round, &body, wire, None)
                    }
                }),
            );
            let (mut handle, host, task, _dir) =
                fixture(app, WireApi::Chat, false, native::Options::default()).await;
            let mut ws = if matches!(delivery, Delivery::WebSocket) {
                Some(socket(&handle).await)
            } else {
                None
            };
            let mut body = json!({"model":"m","stream":!matches!(delivery,Delivery::Json),"store":false,"tools":[search()],"input":[{"role":"user","content":"Read the fixture and report its result."}]});
            let (status, events, raw) = exchange(&handle, &mut ws, body.clone(), delivery).await;
            let first = completed(status, &events, delivery);
            assert!(!raw.contains("cockpit_client_tool_search"));
            assert!(!raw.contains("response.function_call_arguments"));
            let search_call = &first["output"][0];
            assert_eq!(search_call["type"], "tool_search_call");
            assert_eq!(search_call["execution"], "client");
            assert_eq!(search_call["call_id"], "call_search");
            assert_eq!(search_call["arguments"], json!({"query":"查找文件"}));
            assert!(search_call["id"].as_str().unwrap().starts_with("tsc_"));
            assert_eq!(first["usage"]["total_tokens"], 5);
            body["input"].as_array_mut().unwrap().extend([
                search_call.clone(),
                json!({"type":"tool_search_output","execution":"client","status":"completed","call_id":"call_search","tools":discovered()}),
            ]);
            let (status, events, raw) = exchange(&handle, &mut ws, body.clone(), delivery).await;
            let second = completed(status, &events, delivery);
            assert!(!raw.contains("cockpit_client_tool_search"));
            assert_eq!(second["output"][0]["namespace"], "workspace");
            assert_eq!(second["output"][0]["name"], "read_file");
            assert_eq!(second["output"][0]["call_id"], "call_read");
            assert_eq!(
                second["output"][0]["arguments"],
                "{\"path\":\"fixture.txt\"}"
            );
            assert_eq!(second["output"][1]["name"], "observe");
            assert_eq!(second["output"][1]["call_id"], "call_observe");
            body["input"]
                .as_array_mut()
                .unwrap()
                .extend(second["output"].as_array().unwrap().iter().cloned());
            body["input"].as_array_mut().unwrap().extend([
                json!({"type":"function_call_output","call_id":"call_read","output":"fixture contents"}),
                json!({"type":"function_call_output","call_id":"call_observe","output":"observation complete"}),
            ]);
            let (status, events, raw) = exchange(&handle, &mut ws, body, delivery).await;
            let third = completed(status, &events, delivery);
            assert!(!raw.contains("cockpit_client_tool_search"));
            assert_eq!(
                third["output"][0]["content"][0]["text"],
                "Fixture tool results received."
            );
            {
                let requests = captures.lock().unwrap();
                assert_eq!(requests.len(), 3, "{delivery:?} {wire:?}");
                for request in requests.iter() {
                    assert!(request.get("input").is_none());
                    assert!(request["tools"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|tool| tool["type"] == "function"));
                    assert!(!request.to_string().contains("\"type\":\"tool_search"));
                    assert_eq!(
                        request["messages"][0]["content"],
                        "Read the fixture and report its result."
                    );
                }
                let second_messages = requests[1]["messages"].as_array().unwrap();
                assert!(second_messages
                    .iter()
                    .any(|message| message["role"] == "assistant"
                        && message["tool_calls"][0]["id"] == "call_search"
                        && message["tool_calls"][0]["function"]["name"]
                            == search_name(&requests[1])));
                assert!(second_messages
                    .iter()
                    .any(|message| message["role"] == "tool"
                        && message["tool_call_id"] == "call_search"
                        && message["content"] == discovered().to_string()));
                let read = requests[1]["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|tool| tool["function"]["name"] == "workspace__read_file")
                    .unwrap();
                assert_eq!(
                    read["function"]["parameters"],
                    discovered()[0]["tools"][0]["parameters"]
                );
                assert_eq!(read["function"]["strict"], true);
                let messages = requests[2]["messages"].as_array().unwrap();
                for (id, result) in [
                    ("call_read", "fixture contents"),
                    ("call_observe", "observation complete"),
                ] {
                    assert!(messages.iter().any(|message| message["role"] == "tool"
                        && message["tool_call_id"] == id
                        && message["content"] == result));
                }
                assert!(messages.iter().any(|message| message["tool_calls"]
                    .as_array()
                    .is_some_and(|calls| calls.iter().any(|call| call["id"] == "call_read"
                        && call["function"]["name"] == "workspace__read_file"))));
            }
            reports(&host, 3).await;
            assert!(host
                .reports
                .lock()
                .unwrap()
                .iter()
                .all(|report| report.success));
            drop(ws);
            handle.shutdown(Duration::from_secs(2)).await.unwrap();
            task.abort();
        }
    }
}

#[tokio::test]
async fn chat_client_discovery_invalid_search_never_succeeds_or_saves_a_checkpoint() {
    for delivery in [Delivery::Json, Delivery::Sse, Delivery::WebSocket] {
        for wire in [Upstream::Json, Upstream::SseDone, Upstream::SseEof] {
            for arguments in ["[]", "{\"query\":"] {
                let app = Router::new().route(
                    "/v1/chat/completions",
                    post(move |Json(body): Json<Value>| async move {
                        chat_reply(1, &body, wire, Some(arguments))
                    }),
                );
                let (mut handle, host, task, dir) =
                    fixture(app, WireApi::Chat, false, native::Options::default()).await;
                let mut ws = if matches!(delivery, Delivery::WebSocket) {
                    Some(socket(&handle).await)
                } else {
                    None
                };
                let body = json!({"model":"m","stream":!matches!(delivery,Delivery::Json),"tools":[search()],"input":[{"role":"user","content":"Find a fixture reader."}]});
                let (status, events, raw) = exchange(&handle, &mut ws, body, delivery).await;
                if matches!(delivery, Delivery::Json) {
                    assert!(!status.unwrap().is_success(), "{wire:?} {arguments}");
                }
                assert!(!events.is_empty());
                assert!(!raw.contains("cockpit_client_tool_search"));
                assert!(
                    events
                        .iter()
                        .all(|event| event["type"] != "response.completed"
                            && event["type"] != "response.output_item.done"
                            && event["type"] != "response.function_call_arguments.done"),
                    "{delivery:?} {wire:?} {arguments}: {events:?}"
                );
                let last = events.last().unwrap();
                assert!(
                    last.get("error").is_some() || last.pointer("/response/error").is_some(),
                    "{last}"
                );
                reports(&host, 1).await;
                assert!(!host.reports.lock().unwrap()[0].success);
                let connection =
                    rusqlite::Connection::open(dir.path().join("identity.sqlite")).unwrap();
                for query in [
                    "SELECT COUNT(*) FROM native_opaque_v1 WHERE kind='response'",
                    "SELECT COUNT(*) FROM native_rebuild_checkpoints_v1",
                    "SELECT COUNT(*) FROM native_rebuild_portable_v1",
                ] {
                    let count: i64 = connection.query_row(query, [], |row| row.get(0)).unwrap();
                    assert_eq!(count, 0, "{delivery:?} {wire:?} {arguments}: {query}");
                }
                drop(connection);
                drop(ws);
                handle.shutdown(Duration::from_secs(2)).await.unwrap();
                task.abort();
            }
        }
    }
}
