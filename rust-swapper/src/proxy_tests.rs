use crate::{
    config::Config, create_compact_router, create_main_router, pipeline::RequestPipeline,
    process::ProcessManager, AppState,
};
use axum::{
    body::{Body, Bytes},
    http::{Method, Request, StatusCode, Uri},
    Json, Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::{net::TcpListener, sync::Mutex, task::JoinHandle};
use tower::ServiceExt;

struct EchoBackend {
    port: u16,
    task: JoinHandle<()>,
}

impl EchoBackend {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().fallback(echo_request);
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { port, task }
    }
}

impl Drop for EchoBackend {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn echo_request(method: Method, uri: Uri, body: Bytes) -> Json<Value> {
    let payload = serde_json::from_slice::<Value>(&body).ok();
    Json(json!({
        "method": method.as_str(),
        "target": uri.to_string(),
        "payload": payload,
    }))
}

enum ProxyKind {
    Main,
    Compact,
}

async fn assert_forwarded_target(kind: ProxyKind, method: Method, target: &str) {
    let backend = EchoBackend::start().await;
    let mut config = Config::from_env();
    config.port_main_internal = backend.port;
    config.port_compact_internal = backend.port;
    let active_model = match kind {
        ProxyKind::Main => "main",
        ProxyKind::Compact => "compact",
    };
    let state = AppState {
        current_main_model: Arc::new(Mutex::new(config.model_main_path.clone())),
        config,
        process_mgr: ProcessManager::new(),
        http_client: reqwest::Client::new(),
        swap_lock: Arc::new(Mutex::new(())),
        last_active_model: Arc::new(Mutex::new(active_model.to_string())),
        pipeline: Arc::new(RequestPipeline::build_default()),
    };
    let app = match kind {
        ProxyKind::Main => create_main_router(state),
        ProxyKind::Compact => create_compact_router(state),
    };
    let body = if method == Method::POST {
        Body::from(
            json!({
                "model": "any",
                "messages": [{ "role": "user", "content": "Hello" }],
            })
            .to_string(),
        )
    } else {
        Body::empty()
    };
    let request = Request::builder()
        .method(method.clone())
        .uri(target)
        .header("Content-Type", "application/json")
        .body(body)
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let echoed: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(echoed["target"], target);
    assert_eq!(echoed["method"], method.as_str());
    if method == Method::POST {
        assert_eq!(echoed["payload"]["messages"][0]["content"], "Hello");
    }
}

#[tokio::test]
async fn main_proxy_preserves_repeated_and_empty_query_parameters() {
    assert_forwarded_target(
        ProxyKind::Main,
        Method::GET,
        "/v1/chat/completions?tag=alpha&tag=beta&empty=&flag",
    )
    .await;
}

#[tokio::test]
async fn compact_proxy_preserves_repeated_and_empty_query_parameters() {
    assert_forwarded_target(
        ProxyKind::Compact,
        Method::GET,
        "/v1/chat/completions?tag=alpha&tag=beta&empty=&flag",
    )
    .await;
}

#[tokio::test]
async fn main_proxy_preserves_encoded_post_target() {
    assert_forwarded_target(
        ProxyKind::Main,
        Method::POST,
        "/v1/custom%2Fendpoint?space=hello%20world&plus=a+b&value=%2F%3F%26&unicode=%ED%95%9C",
    )
    .await;
}

#[tokio::test]
async fn compact_proxy_preserves_encoded_post_target() {
    assert_forwarded_target(
        ProxyKind::Compact,
        Method::POST,
        "/v1/custom%2Fendpoint?space=hello%20world&plus=a+b&value=%2F%3F%26&unicode=%ED%95%9C",
    )
    .await;
}

#[tokio::test]
async fn main_proxy_preserves_queryless_target() {
    assert_forwarded_target(ProxyKind::Main, Method::GET, "/v1/chat/completions").await;
}

#[tokio::test]
async fn compact_proxy_preserves_queryless_post_target() {
    assert_forwarded_target(ProxyKind::Compact, Method::POST, "/v1/chat/completions").await;
}
