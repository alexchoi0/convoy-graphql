use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header, Method, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tower_http::cors::{Any, CorsLayer};

use super::BuiltSchema;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    ConnectionInit {
        #[serde(default)]
        payload: Option<serde_json::Value>,
    },
    ConnectionAck {
        #[serde(skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>,
    },
    Ping {
        #[serde(skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>,
    },
    Pong {
        #[serde(skip_serializing_if = "Option::is_none")]
        payload: Option<serde_json::Value>,
    },
    Subscribe {
        id: String,
        payload: SubscribePayload,
    },
    Next {
        id: String,
        payload: serde_json::Value,
    },
    Error {
        id: String,
        payload: Vec<serde_json::Value>,
    },
    Complete {
        id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribePayload {
    pub query: String,
    #[serde(default)]
    pub variables: Option<serde_json::Value>,
    #[serde(default, rename = "operationName")]
    pub operation_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GraphQLRequest {
    pub query: String,
    #[serde(default)]
    pub variables: Option<serde_json::Value>,
    #[serde(default, rename = "operationName")]
    pub operation_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GraphQLResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<serde_json::Value>,
}

struct AppState {
    schema: BuiltSchema,
}

pub struct GraphQLServer {
    schema: BuiltSchema,
}

impl GraphQLServer {
    pub fn new(schema: BuiltSchema) -> Self {
        Self { schema }
    }

    pub async fn serve(self, addr: &str) -> Result<(), std::io::Error> {
        let addr: SocketAddr = addr.parse().map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Invalid address: {}", e),
            )
        })?;

        let state = Arc::new(AppState {
            schema: self.schema,
        });

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE, header::ACCEPT]);

        let app = Router::new()
            .route("/graphql", get(graphql_get_handler))
            .route("/graphql", post(graphql_post_handler))
            .route("/health", get(health_handler))
            .layer(cors)
            .with_state(state);

        println!("🚀 GraphQL server running at http://{}/graphql", addr);
        println!("   Playground available at http://{}/graphql", addr);
        println!("   WebSocket subscriptions at ws://{}/graphql", addr);

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await
    }

    pub fn router(self) -> Router {
        let state = Arc::new(AppState {
            schema: self.schema,
        });

        Router::new()
            .route("/graphql", get(graphql_get_handler))
            .route("/graphql", post(graphql_post_handler))
            .route("/health", get(health_handler))
            .with_state(state)
    }
}

async fn graphql_get_handler(
    ws: Option<WebSocketUpgrade>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Some(ws) = ws {
        return ws
            .protocols(["graphql-transport-ws"])
            .on_upgrade(move |socket| handle_socket(socket, state))
            .into_response();
    }

    Html(PLAYGROUND_HTML).into_response()
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Message>(32);

    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    let mut initialized = false;
    let mut subscriptions: std::collections::HashMap<String, tokio::task::JoinHandle<()>> =
        std::collections::HashMap::new();

    while let Some(result) = receiver.next().await {
        let msg = match result {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(_) => break,
        };

        let ws_msg: WsMessage = match serde_json::from_str(&msg) {
            Ok(m) => m,
            Err(e) => {
                let _ = tx
                    .send(Message::Text(
                        serde_json::to_string(&serde_json::json!({
                            "type": "error",
                            "payload": [{"message": format!("Invalid message: {}", e)}]
                        }))
                        .unwrap(),
                    ))
                    .await;
                continue;
            }
        };

        match ws_msg {
            WsMessage::ConnectionInit { .. } => {
                initialized = true;
                let ack = WsMessage::ConnectionAck { payload: None };
                let _ = tx
                    .send(Message::Text(serde_json::to_string(&ack).unwrap()))
                    .await;
            }

            WsMessage::Ping { payload } => {
                let pong = WsMessage::Pong { payload };
                let _ = tx
                    .send(Message::Text(serde_json::to_string(&pong).unwrap()))
                    .await;
            }

            WsMessage::Subscribe { id, payload } if initialized => {
                let schema = state.schema.clone();
                let tx = tx.clone();
                let sub_id = id.clone();

                if let Some(handle) = subscriptions.remove(&id) {
                    handle.abort();
                }

                let handle = tokio::spawn(async move {
                    execute_subscription(schema, sub_id, payload, tx).await;
                });

                subscriptions.insert(id, handle);
            }

            WsMessage::Complete { id } => {
                if let Some(handle) = subscriptions.remove(&id) {
                    handle.abort();
                }
            }

            _ => {}
        }
    }

    for (_, handle) in subscriptions {
        handle.abort();
    }

    send_task.abort();
}

async fn execute_subscription(
    schema: BuiltSchema,
    id: String,
    payload: SubscribePayload,
    tx: mpsc::Sender<Message>,
) {
    let mut request = async_graphql::Request::new(&payload.query);

    if let Some(vars) = payload.variables {
        if let Ok(variables) = serde_json::from_value(vars) {
            request = request.variables(variables);
        }
    }

    if let Some(op_name) = payload.operation_name {
        request = request.operation_name(op_name);
    }

    let mut stream = schema.graphql_schema.execute_stream(request);

    while let Some(response) = stream.next().await {
        let data = response.data.into_json().unwrap_or(serde_json::Value::Null);

        if !response.errors.is_empty() {
            let errors: Vec<serde_json::Value> = response
                .errors
                .into_iter()
                .map(|e| {
                    serde_json::json!({
                        "message": e.message,
                        "locations": e.locations,
                        "path": e.path
                    })
                })
                .collect();

            let error_msg = WsMessage::Error {
                id: id.clone(),
                payload: errors,
            };
            if tx
                .send(Message::Text(serde_json::to_string(&error_msg).unwrap()))
                .await
                .is_err()
            {
                break;
            }
        } else {
            let next_msg = WsMessage::Next {
                id: id.clone(),
                payload: serde_json::json!({ "data": data }),
            };
            if tx
                .send(Message::Text(serde_json::to_string(&next_msg).unwrap()))
                .await
                .is_err()
            {
                break;
            }
        }
    }

    let complete_msg = WsMessage::Complete { id };
    let _ = tx
        .send(Message::Text(serde_json::to_string(&complete_msg).unwrap()))
        .await;
}

async fn graphql_post_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<GraphQLRequest>,
) -> impl IntoResponse {
    let mut gql_request = async_graphql::Request::new(&request.query);

    if let Some(vars) = request.variables {
        if let Ok(variables) = serde_json::from_value(vars) {
            gql_request = gql_request.variables(variables);
        }
    }

    if let Some(op_name) = request.operation_name {
        gql_request = gql_request.operation_name(op_name);
    }

    let response = state.schema.execute(gql_request).await;

    let data = if response.data != async_graphql::Value::Null {
        Some(response.data.into_json().unwrap_or(serde_json::Value::Null))
    } else {
        None
    };

    let errors: Vec<serde_json::Value> = response
        .errors
        .into_iter()
        .map(|e| {
            serde_json::json!({
                "message": e.message,
                "locations": e.locations,
                "path": e.path
            })
        })
        .collect();

    let status = if errors.is_empty() || data.is_some() {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };

    (status, Json(GraphQLResponse { data, errors }))
}

async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

const PLAYGROUND_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>GraphQL Playground</title>
    <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/graphql-playground-react/build/static/css/index.css" />
    <link rel="shortcut icon" href="https://cdn.jsdelivr.net/npm/graphql-playground-react/build/favicon.png" />
    <script src="https://cdn.jsdelivr.net/npm/graphql-playground-react/build/static/js/middleware.js"></script>
</head>
<body>
    <div id="root">
        <style>
            body {
                background-color: rgb(23, 42, 58);
                font-family: Open Sans, sans-serif;
                height: 90vh;
            }
            #root {
                height: 100%;
                width: 100%;
                display: flex;
                align-items: center;
                justify-content: center;
            }
            .loading {
                font-size: 32px;
                font-weight: 200;
                color: rgba(255, 255, 255, .6);
                margin-left: 28px;
            }
            img {
                width: 78px;
                height: 78px;
            }
            .title {
                font-weight: 400;
            }
        </style>
        <img src="https://cdn.jsdelivr.net/npm/graphql-playground-react/build/logo.png" alt="">
        <div class="loading">
            Loading <span class="title">GraphQL Playground</span>
        </div>
    </div>
    <script>
        window.addEventListener('load', function() {
            GraphQLPlayground.init(document.getElementById('root'), {
                endpoint: window.location.href,
                settings: {
                    'request.credentials': 'same-origin',
                }
            })
        })
    </script>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use async_graphql::dynamic;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use hyper::Request;
    use tower::ServiceExt;

    fn create_test_schema() -> BuiltSchema {
        let query = dynamic::Object::new("Query")
            .field(dynamic::Field::new(
                "hello",
                dynamic::TypeRef::named_nn(dynamic::TypeRef::STRING),
                |_ctx| {
                    dynamic::FieldFuture::new(async move {
                        Ok(Some(dynamic::FieldValue::value("world")))
                    })
                },
            ))
            .field(
                dynamic::Field::new(
                    "greet",
                    dynamic::TypeRef::named_nn(dynamic::TypeRef::STRING),
                    |ctx| {
                        dynamic::FieldFuture::new(async move {
                            let name = ctx.args.try_get("name")?.string()?;
                            Ok(Some(dynamic::FieldValue::value(format!(
                                "Hello, {}!",
                                name
                            ))))
                        })
                    },
                )
                .argument(dynamic::InputValue::new(
                    "name",
                    dynamic::TypeRef::named_nn(dynamic::TypeRef::STRING),
                )),
            )
            .field(
                dynamic::Field::new(
                    "add",
                    dynamic::TypeRef::named_nn(dynamic::TypeRef::INT),
                    |ctx| {
                        dynamic::FieldFuture::new(async move {
                            let a = ctx.args.try_get("a")?.i64()?;
                            let b = ctx.args.try_get("b")?.i64()?;
                            Ok(Some(dynamic::FieldValue::value(a + b)))
                        })
                    },
                )
                .argument(dynamic::InputValue::new(
                    "a",
                    dynamic::TypeRef::named_nn(dynamic::TypeRef::INT),
                ))
                .argument(dynamic::InputValue::new(
                    "b",
                    dynamic::TypeRef::named_nn(dynamic::TypeRef::INT),
                )),
            );

        let schema = dynamic::Schema::build("Query", None, None)
            .register(query)
            .finish()
            .unwrap();

        BuiltSchema::from_dynamic_schema(schema)
    }

    async fn graphql_post(app: &Router, body: &str) -> (StatusCode, serde_json::Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/graphql")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (status, json)
    }

    #[tokio::test]
    async fn test_graphql_request_parsing() {
        let json = r#"{"query": "{ hello }"}"#;
        let request: GraphQLRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.query, "{ hello }");
        assert!(request.variables.is_none());
        assert!(request.operation_name.is_none());
    }

    #[tokio::test]
    async fn test_graphql_request_with_variables() {
        let json = r#"{"query": "query($id: ID!) { user(id: $id) }", "variables": {"id": "123"}}"#;
        let request: GraphQLRequest = serde_json::from_str(json).unwrap();
        assert!(request.variables.is_some());
        assert_eq!(
            request.variables.unwrap().get("id").unwrap(),
            &serde_json::json!("123")
        );
    }

    #[tokio::test]
    async fn test_graphql_request_with_operation_name() {
        let json = r#"{"query": "query GetUser { user }", "operationName": "GetUser"}"#;
        let request: GraphQLRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.operation_name, Some("GetUser".to_string()));
    }

    #[tokio::test]
    async fn test_simple_query() {
        let schema = create_test_schema();
        let app = GraphQLServer::new(schema).router();

        let (status, json) = graphql_post(&app, r#"{"query": "{ hello }"}"#).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["data"]["hello"], "world");
    }

    #[tokio::test]
    async fn test_query_with_arguments() {
        let schema = create_test_schema();
        let app = GraphQLServer::new(schema).router();

        let (status, json) = graphql_post(&app, r#"{"query": "{ greet(name: \"Alice\") }"}"#).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["data"]["greet"], "Hello, Alice!");
    }

    #[tokio::test]
    async fn test_query_with_variables() {
        let schema = create_test_schema();
        let app = GraphQLServer::new(schema).router();

        let body = r#"{
            "query": "query Add($a: Int!, $b: Int!) { add(a: $a, b: $b) }",
            "variables": {"a": 5, "b": 3}
        }"#;

        let (status, json) = graphql_post(&app, body).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["data"]["add"], 8);
    }

    #[tokio::test]
    async fn test_invalid_query_returns_error() {
        let schema = create_test_schema();
        let app = GraphQLServer::new(schema).router();

        let (status, json) = graphql_post(&app, r#"{"query": "{ nonexistent }"}"#).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["errors"].is_array());
        assert!(!json["errors"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_syntax_error_returns_error() {
        let schema = create_test_schema();
        let app = GraphQLServer::new(schema).router();

        let (status, json) = graphql_post(&app, r#"{"query": "{ hello"}"#).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["errors"].is_array());
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let schema = create_test_schema();
        let app = GraphQLServer::new(schema).router();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
    }

    #[tokio::test]
    async fn test_playground_returns_html() {
        let schema = create_test_schema();
        let app = GraphQLServer::new(schema).router();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/graphql")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("GraphQL Playground"));
    }

    #[test]
    fn test_ws_message_connection_init_serialization() {
        let msg = WsMessage::ConnectionInit { payload: None };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("connection_init"));
    }

    #[test]
    fn test_ws_message_subscribe_deserialization() {
        let json = r#"{
            "type": "subscribe",
            "id": "1",
            "payload": {
                "query": "subscription { counter }"
            }
        }"#;

        let msg: WsMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsMessage::Subscribe { id, payload } => {
                assert_eq!(id, "1");
                assert_eq!(payload.query, "subscription { counter }");
            }
            _ => panic!("Expected Subscribe message"),
        }
    }

    #[test]
    fn test_ws_message_complete_serialization() {
        let msg = WsMessage::Complete {
            id: "sub-1".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("complete"));
        assert!(json.contains("sub-1"));
    }

    #[test]
    fn test_graphql_response_serialization_with_data() {
        let response = GraphQLResponse {
            data: Some(serde_json::json!({"hello": "world"})),
            errors: vec![],
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("hello"));
        assert!(!json.contains("errors"));
    }

    #[test]
    fn test_graphql_response_serialization_with_errors() {
        let response = GraphQLResponse {
            data: None,
            errors: vec![serde_json::json!({"message": "Something went wrong"})],
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("errors"));
        assert!(json.contains("Something went wrong"));
        assert!(!json.contains("data"));
    }
}
