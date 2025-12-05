use std::time::Duration;

use async_graphql::dynamic;
use convoy_graphql::server::BuiltSchema;
use convoy_graphql::GraphQLServer;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

fn create_test_schema() -> BuiltSchema {
    let query = dynamic::Object::new("Query")
        .field(dynamic::Field::new(
            "hello",
            dynamic::TypeRef::named_nn(dynamic::TypeRef::STRING),
            |_ctx| {
                dynamic::FieldFuture::new(
                    async move { Ok(Some(dynamic::FieldValue::value("world"))) },
                )
            },
        ))
        .field(
            dynamic::Field::new(
                "echo",
                dynamic::TypeRef::named_nn(dynamic::TypeRef::STRING),
                |ctx| {
                    dynamic::FieldFuture::new(async move {
                        let msg = ctx.args.try_get("message")?.string()?;
                        Ok(Some(dynamic::FieldValue::value(msg.to_string())))
                    })
                },
            )
            .argument(dynamic::InputValue::new(
                "message",
                dynamic::TypeRef::named_nn(dynamic::TypeRef::STRING),
            )),
        )
        .field(
            dynamic::Field::new(
                "sum",
                dynamic::TypeRef::named_nn(dynamic::TypeRef::INT),
                |ctx| {
                    dynamic::FieldFuture::new(async move {
                        let numbers: Vec<i64> = ctx
                            .args
                            .try_get("numbers")?
                            .list()?
                            .iter()
                            .map(|v| v.i64())
                            .collect::<Result<Vec<_>, _>>()?;
                        Ok(Some(dynamic::FieldValue::value(
                            numbers.iter().sum::<i64>(),
                        )))
                    })
                },
            )
            .argument(dynamic::InputValue::new(
                "numbers",
                dynamic::TypeRef::named_nn_list_nn(dynamic::TypeRef::INT),
            )),
        );

    let subscription =
        dynamic::Subscription::new("Subscription").field(dynamic::SubscriptionField::new(
            "countdown",
            dynamic::TypeRef::named_nn(dynamic::TypeRef::INT),
            |_ctx| {
                dynamic::SubscriptionFieldFuture::new(async move {
                    let stream = async_stream::stream! {
                        for i in (1..=3).rev() {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            yield Ok(dynamic::FieldValue::value(i));
                        }
                    };
                    Ok(stream)
                })
            },
        ));

    let schema = dynamic::Schema::build("Query", None, Some("Subscription"))
        .register(query)
        .register(subscription)
        .finish()
        .unwrap();

    BuiltSchema::from_dynamic_schema(schema)
}

async fn find_available_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

async fn start_test_server() -> (String, tokio::task::JoinHandle<()>) {
    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{}", port);
    let base_url = format!("http://{}", addr);

    let schema = create_test_schema();
    let server = GraphQLServer::new(schema);

    let handle = tokio::spawn(async move {
        let _ = server.serve(&addr).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    (base_url, handle)
}

#[tokio::test]
async fn test_e2e_simple_query() {
    let (base_url, handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{}/graphql", base_url))
        .json(&json!({
            "query": "{ hello }"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["data"]["hello"], "world");

    handle.abort();
}

#[tokio::test]
async fn test_e2e_query_with_argument() {
    let (base_url, handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{}/graphql", base_url))
        .json(&json!({
            "query": r#"{ echo(message: "Hello, E2E!") }"#
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["data"]["echo"], "Hello, E2E!");

    handle.abort();
}

#[tokio::test]
async fn test_e2e_query_with_variables() {
    let (base_url, handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{}/graphql", base_url))
        .json(&json!({
            "query": "query Echo($msg: String!) { echo(message: $msg) }",
            "variables": { "msg": "Variable test" }
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["data"]["echo"], "Variable test");

    handle.abort();
}

#[tokio::test]
async fn test_e2e_query_with_list_argument() {
    let (base_url, handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{}/graphql", base_url))
        .json(&json!({
            "query": "{ sum(numbers: [1, 2, 3, 4, 5]) }"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["data"]["sum"], 15);

    handle.abort();
}

#[tokio::test]
async fn test_e2e_invalid_query() {
    let (base_url, handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{}/graphql", base_url))
        .json(&json!({
            "query": "{ nonExistentField }"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 400);

    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["errors"].is_array());
    assert!(!body["errors"].as_array().unwrap().is_empty());

    handle.abort();
}

#[tokio::test]
async fn test_e2e_health_check() {
    let (base_url, handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let response = client
        .get(format!("{}/health", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["status"], "ok");

    handle.abort();
}

#[tokio::test]
async fn test_e2e_playground_html() {
    let (base_url, handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let response = client
        .get(format!("{}/graphql", base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    let body = response.text().await.unwrap();
    assert!(body.contains("<!DOCTYPE html>"));
    assert!(body.contains("GraphQL Playground"));

    handle.abort();
}

#[tokio::test]
async fn test_e2e_multiple_queries_same_connection() {
    let (base_url, handle) = start_test_server().await;
    let client = reqwest::Client::new();

    let response1 = client
        .post(format!("{}/graphql", base_url))
        .json(&json!({"query": "{ hello }"}))
        .send()
        .await
        .unwrap();
    let body1: serde_json::Value = response1.json().await.unwrap();
    assert_eq!(body1["data"]["hello"], "world");

    let response2 = client
        .post(format!("{}/graphql", base_url))
        .json(&json!({"query": r#"{ echo(message: "test") }"#}))
        .send()
        .await
        .unwrap();
    let body2: serde_json::Value = response2.json().await.unwrap();
    assert_eq!(body2["data"]["echo"], "test");

    let response3 = client
        .post(format!("{}/graphql", base_url))
        .json(&json!({"query": "{ sum(numbers: [10, 20]) }"}))
        .send()
        .await
        .unwrap();
    let body3: serde_json::Value = response3.json().await.unwrap();
    assert_eq!(body3["data"]["sum"], 30);

    handle.abort();
}

#[tokio::test]
async fn test_e2e_websocket_subscription() {
    let (base_url, handle) = start_test_server().await;
    let ws_url = base_url.replace("http://", "ws://") + "/graphql";

    let (mut ws_stream, _) = connect_async(&ws_url).await.expect("Failed to connect");

    let init_msg = json!({"type": "connection_init"});
    ws_stream
        .send(Message::Text(init_msg.to_string().into()))
        .await
        .unwrap();

    let ack = ws_stream.next().await.unwrap().unwrap();
    let ack_json: serde_json::Value = serde_json::from_str(ack.to_text().unwrap()).unwrap();
    assert_eq!(ack_json["type"], "connection_ack");

    let subscribe_msg = json!({
        "type": "subscribe",
        "id": "1",
        "payload": {
            "query": "subscription { countdown }"
        }
    });
    ws_stream
        .send(Message::Text(subscribe_msg.to_string().into()))
        .await
        .unwrap();

    let mut results = Vec::new();
    let timeout = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(msg) = ws_stream.next().await {
            let msg = msg.unwrap();
            if let Message::Text(text) = msg {
                let json: serde_json::Value = serde_json::from_str(&text).unwrap();
                match json["type"].as_str() {
                    Some("next") => {
                        results.push(json["payload"]["data"]["countdown"].as_i64().unwrap());
                    }
                    Some("complete") => break,
                    _ => {}
                }
            }
        }
    });

    timeout.await.expect("Subscription timed out");

    assert_eq!(results, vec![3, 2, 1]);

    handle.abort();
}

#[tokio::test]
async fn test_e2e_websocket_ping_pong() {
    let (base_url, handle) = start_test_server().await;
    let ws_url = base_url.replace("http://", "ws://") + "/graphql";

    let (mut ws_stream, _) = connect_async(&ws_url).await.expect("Failed to connect");

    let init_msg = json!({"type": "connection_init"});
    ws_stream
        .send(Message::Text(init_msg.to_string().into()))
        .await
        .unwrap();

    let _ = ws_stream.next().await.unwrap().unwrap();

    let ping_msg = json!({"type": "ping", "payload": {"test": "data"}});
    ws_stream
        .send(Message::Text(ping_msg.to_string().into()))
        .await
        .unwrap();

    let pong = ws_stream.next().await.unwrap().unwrap();
    let pong_json: serde_json::Value = serde_json::from_str(pong.to_text().unwrap()).unwrap();
    assert_eq!(pong_json["type"], "pong");
    assert_eq!(pong_json["payload"]["test"], "data");

    handle.abort();
}

#[tokio::test]
async fn test_e2e_websocket_complete_subscription() {
    let (base_url, handle) = start_test_server().await;
    let ws_url = base_url.replace("http://", "ws://") + "/graphql";

    let (mut ws_stream, _) = connect_async(&ws_url).await.expect("Failed to connect");

    ws_stream
        .send(Message::Text(
            json!({"type": "connection_init"}).to_string().into(),
        ))
        .await
        .unwrap();

    let _ = ws_stream.next().await.unwrap().unwrap();

    let subscribe_msg = json!({
        "type": "subscribe",
        "id": "sub-1",
        "payload": {
            "query": "subscription { countdown }"
        }
    });
    ws_stream
        .send(Message::Text(subscribe_msg.to_string().into()))
        .await
        .unwrap();

    let first = ws_stream.next().await.unwrap().unwrap();
    let first_json: serde_json::Value = serde_json::from_str(first.to_text().unwrap()).unwrap();
    assert_eq!(first_json["type"], "next");

    let complete_msg = json!({
        "type": "complete",
        "id": "sub-1"
    });
    ws_stream
        .send(Message::Text(complete_msg.to_string().into()))
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    handle.abort();
}
