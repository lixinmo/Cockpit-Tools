//! Replay the Go Chat adapter's saved Responses history through Native's actual
//! HTTP/WS gateway, including a fresh tool round and a model switch.
use super::*;

#[derive(Clone, Copy, Debug)]
enum Delivery {
    Json,
    Sse,
    WebSocket,
}

const HISTORY_ROUNDS: usize = 443;
const OLD_ARGUMENTS: &str = "{\"path\":\"旧文件.txt\",\"limit\":7}";
const OLD_PATCH: &str = "*** Begin Patch\n*** Add File: old.txt\n+历史内容\n*** End Patch";
const NEW_ARGUMENTS: &str = "{\"path\":\"next.txt\",\"limit\":9}";
const NEW_PATCH: &str = "*** Begin Patch\n*** Add File: next.txt\n+new content\n*** End Patch";

fn tools() -> Value {
    json!([
        {"type":"function","name":"read_fixture","parameters":{"type":"object","properties":{"path":{"type":"string"},"limit":{"type":"integer"}},"required":["path","limit"]}},
        {"type":"custom","name":"apply_patch","format":{"type":"text"}}
    ])
}

fn message(role: &str, text: String) -> Value {
    json!({"type":"message","role":role,"content":[{"type":if role=="assistant" {"output_text"} else {"input_text"},"text":text}]})
}

fn saved_history() -> Value {
    let mut input = Vec::new();
    // Generated fixture data only. A long resumed transcript must preserve every
    // visible turn and summary, not just succeed with a new empty conversation.
    for index in 0..HISTORY_ROUNDS {
        input.push(message("user", format!("saved user turn {index}")));
        input.push(json!({"type":"reasoning","id":format!("rs_go_{index}"),"encrypted_content":"","summary":[{"type":"summary_text","text":format!("saved summary {index}")}]}));
        input.push(message(
            "assistant",
            format!("saved assistant turn {index}"),
        ));
    }
    input.extend([
        json!({"type":"function_call","id":"fc_old_read","call_id":"old_read","name":"read_fixture","arguments":OLD_ARGUMENTS}),
        json!({"type":"function_call_output","call_id":"old_read","output":"old file contents\n第二行"}),
        json!({"type":"custom_tool_call","id":"fc_old_patch","call_id":"old_patch","name":"apply_patch","input":OLD_PATCH}),
        json!({"type":"custom_tool_call_output","call_id":"old_patch","output":"old patch applied"}),
        message("user", "Resume this conversation and use both tools.".into()),
    ]);
    json!(input)
}

fn reply(round: usize, stream: bool) -> Response {
    let calls = json!([
        {"id":"next_read","type":"function","function":{"name":"read_fixture","arguments":NEW_ARGUMENTS}},
        {"id":"next_patch","type":"function","function":{"name":"apply_patch","arguments":json!({"input":NEW_PATCH}).to_string()}}
    ]);
    let mut message = json!({"role":"assistant","content":"Both tool results received."});
    let reason = if round == 1 { "tool_calls" } else { "stop" };
    if round == 1 {
        message["content"] = Value::Null;
        message["tool_calls"] = calls.clone();
    }
    if !stream {
        return Response::builder()
            .header("content-type", "application/json")
            .body(Body::from(json!({"id":format!("chat_legacy_{round}"),"model":"fixture","choices":[{"index":0,"message":message,"finish_reason":reason}]}).to_string()))
            .unwrap();
    }
    let delta = if round == 1 {
        let mut indexed = calls.as_array().unwrap().clone();
        for (index, call) in indexed.iter_mut().enumerate() {
            call["index"] = json!(index);
        }
        json!({"tool_calls":indexed})
    } else {
        json!({"content":"Both tool results received."})
    };
    let chunk = json!({"id":format!("chat_legacy_{round}"),"choices":[{"index":0,"delta":delta,"finish_reason":reason}]});
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from(format!("data: {chunk}\n\ndata: [DONE]\n\n")))
        .unwrap()
}

async fn exchange(
    handle: &native::Handle,
    ws: &mut Option<Socket>,
    mut body: Value,
    delivery: Delivery,
) -> (Option<StatusCode>, Value) {
    if matches!(delivery, Delivery::WebSocket) {
        body["type"] = json!("response.create");
        let ws = ws.as_mut().unwrap();
        send(ws, body).await;
        let event = terminal(ws).await;
        return (None, event);
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
    let raw = response.text().await.unwrap();
    if matches!(delivery, Delivery::Json) || !status.is_success() {
        return (Some(status), serde_json::from_str(&raw).unwrap());
    }
    let last = raw
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .last()
        .expect("SSE has a terminal event");
    (Some(status), last)
}

fn completed(status: Option<StatusCode>, event: Value, delivery: Delivery) -> Value {
    assert!(status.is_none_or(|status| status.is_success()), "{event}");
    let response = if matches!(delivery, Delivery::Json) {
        event
    } else {
        assert_eq!(event["type"], "response.completed", "{event}");
        event["response"].clone()
    };
    assert_eq!(response["status"], "completed");
    response
}

fn assert_call_and_result(messages: &[Value], id: &str, name: &str, args: &str, result: &str) {
    let calls: Vec<_> = messages
        .iter()
        .flat_map(|message| message["tool_calls"].as_array().into_iter().flatten())
        .filter(|call| call["id"] == id)
        .collect();
    assert_eq!(calls.len(), 1, "exactly one call for {id}");
    assert_eq!(calls[0]["function"]["name"], name);
    assert_eq!(calls[0]["function"]["arguments"], args);
    let results: Vec<_> = messages
        .iter()
        .filter(|message| message["role"] == "tool" && message["tool_call_id"] == id)
        .collect();
    assert_eq!(results.len(), 1, "exactly one result for {id}");
    assert_eq!(results[0]["content"], result);
}

fn assert_saved_history(request: &Value) {
    assert!(request.get("input").is_none());
    let messages = request["messages"].as_array().unwrap();
    for index in 0..HISTORY_ROUNDS {
        assert_eq!(messages[index * 2]["role"], "user");
        assert_eq!(
            messages[index * 2]["content"],
            json!([{"type":"text","text":format!("saved user turn {index}")}])
        );
        let assistant = &messages[index * 2 + 1];
        assert_eq!(assistant["role"], "assistant");
        assert_eq!(
            assistant["content"],
            json!([{"type":"text","text":format!("saved assistant turn {index}")}])
        );
        assert_eq!(
            assistant["reasoning_content"],
            format!("saved summary {index}")
        );
    }
    assert_call_and_result(
        messages,
        "old_read",
        "read_fixture",
        OLD_ARGUMENTS,
        "old file contents\n第二行",
    );
    assert_call_and_result(
        messages,
        "old_patch",
        "apply_patch",
        &json!({"input":OLD_PATCH}).to_string(),
        "old patch applied",
    );
    assert!(messages.iter().any(|message| message["role"] == "user"
        && message["content"]
            == json!([{"type":"text","text":"Resume this conversation and use both tools."}])));
    assert!(messages
        .iter()
        .all(|message| message.get("encrypted_content").is_none()));
    assert_eq!(request["tools"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn chat_legacy_reasoning_resumes_long_go_history_and_tool_loop_over_json_sse_websocket() {
    for delivery in [Delivery::Json, Delivery::Sse, Delivery::WebSocket] {
        for (first_model, second_model) in [
            ("step-5-preview", "step-3.7-flash"),
            ("step-3.7-flash", "step-5-preview"),
        ] {
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
                        reply(round, body["stream"] == true)
                    }
                }),
            );
            let (mut handle, host, task, dir) =
                fixture(app, WireApi::Chat, false, native::Options::default()).await;
            let mut ws = if matches!(delivery, Delivery::WebSocket) {
                Some(socket(&handle).await)
            } else {
                None
            };
            let saved = saved_history();
            let mut body = json!({"model":first_model,"stream":!matches!(delivery,Delivery::Json),"store":false,"client_metadata":{"session_id":"resumed-go-thread","thread_id":"resumed-go-thread"},"tools":tools(),"input":saved});
            let (status, event) = exchange(&handle, &mut ws, body.clone(), delivery).await;
            let first = completed(status, event, delivery);
            let calls = first["output"].as_array().unwrap();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0]["type"], "function_call");
            assert_eq!(calls[0]["call_id"], "next_read");
            assert_eq!(calls[0]["arguments"], NEW_ARGUMENTS);
            assert_eq!(calls[1]["type"], "custom_tool_call");
            assert_eq!(calls[1]["call_id"], "next_patch");
            assert_eq!(calls[1]["input"], NEW_PATCH);
            // The same resumed conversation may select either model before the
            // client returns tool results. Send its full persisted history.
            body["model"] = json!(second_model);
            body["input"]
                .as_array_mut()
                .unwrap()
                .extend(calls.iter().cloned());
            body["input"].as_array_mut().unwrap().extend([
                json!({"type":"function_call_output","call_id":"next_read","output":"next file contents"}),
                json!({"type":"custom_tool_call_output","call_id":"next_patch","output":"next patch applied"}),
            ]);
            let (status, event) = exchange(&handle, &mut ws, body, delivery).await;
            let second = completed(status, event, delivery);
            assert_eq!(
                second["output"][0]["content"][0]["text"],
                "Both tool results received."
            );
            {
                let requests = captures.lock().unwrap();
                assert_eq!(requests.len(), 2, "{delivery:?}");
                assert_eq!(requests[0]["model"], first_model);
                assert_eq!(requests[1]["model"], second_model);
                for request in requests.iter() {
                    assert_saved_history(request);
                }
                // The first saved call joins the last assistant message; all
                // remaining saved messages and results each appear once.
                assert_eq!(
                    requests[0]["messages"].as_array().unwrap().len(),
                    HISTORY_ROUNDS * 2 + 4
                );
                let messages = requests[1]["messages"].as_array().unwrap();
                assert_eq!(messages.len(), HISTORY_ROUNDS * 2 + 7);
                assert_call_and_result(
                    messages,
                    "next_read",
                    "read_fixture",
                    NEW_ARGUMENTS,
                    "next file contents",
                );
                assert_call_and_result(
                    messages,
                    "next_patch",
                    "apply_patch",
                    &json!({"input":NEW_PATCH}).to_string(),
                    "next patch applied",
                );
            }
            assert_eq!(
                saved,
                saved_history(),
                "the saved history remains unchanged"
            );
            reports(&host, 2).await;
            assert!(host
                .reports
                .lock()
                .unwrap()
                .iter()
                .all(|report| report.success && report.account_id == "a"));
            let connection =
                rusqlite::Connection::open(dir.path().join("identity.sqlite")).unwrap();
            let encrypted: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM native_opaque_v1 WHERE kind='encrypted'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                encrypted, 0,
                "empty Go placeholders cannot acquire ciphertext ownership"
            );
            drop(connection);
            drop(ws);
            handle.shutdown(Duration::from_secs(2)).await.unwrap();
            task.abort();
        }
    }
}

#[tokio::test]
async fn chat_legacy_reasoning_nonempty_ciphertext_is_rejected_before_upstream_on_every_transport()
{
    for delivery in [Delivery::Json, Delivery::Sse, Delivery::WebSocket] {
        let captures = Arc::new(Mutex::new(Vec::<Value>::new()));
        let seen = captures.clone();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let seen = seen.clone();
                async move {
                    seen.lock().unwrap().push(body);
                    reply(2, false)
                }
            }),
        );
        let (mut handle, _host, task, _dir) =
            fixture(app, WireApi::Chat, false, native::Options::default()).await;
        let mut ws = if matches!(delivery, Delivery::WebSocket) {
            Some(socket(&handle).await)
        } else {
            None
        };
        let body = json!({"model":"step-5-preview","stream":!matches!(delivery,Delivery::Json),"input":[{"type":"reasoning","encrypted_content":"synthetic-opaque-reasoning","summary":[{"type":"summary_text","text":"A visible summary does not authorize discarding ciphertext."}]},{"role":"user","content":"continue"}]});
        let saved = body.clone();
        let (status, event) = exchange(&handle, &mut ws, body.clone(), delivery).await;
        assert!(status.is_none_or(|status| !status.is_success()));
        assert_ne!(event["type"], "response.completed");
        assert!(
            event.get("error").is_some() || event.pointer("/response/error").is_some(),
            "{event}"
        );
        assert!(
            captures.lock().unwrap().is_empty(),
            "opaque reasoning must never reach Chat"
        );
        assert_eq!(body, saved);
        drop(ws);
        handle.shutdown(Duration::from_secs(2)).await.unwrap();
        task.abort();
    }
}
