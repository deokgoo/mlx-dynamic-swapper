//! [Reverse Proxy & SSE Streaming Module]
//! 클라이언트의 HTTP 요청을 내부 MLX 서버 포트로 전달하고 응답/스트리밍을 중계합니다.
//! 
//! 💡 [Python vs Rust 비교 학습]
//! - Python:
//!     async with client.stream(method, url, headers=headers, json=data) as resp:
//!         return StreamingResponse(resp.aiter_bytes(), media_type=resp.headers.get("content-type"))
//! - Rust:
//!     reqwest의 `bytes_stream()`을 Axum의 `Body::from_stream`으로 변환하여
//!     비동기 무복사(Zero-copy)에 가까운 초고속 SSE 중계를 실현합니다.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, Method, StatusCode};
use axum::response::Response;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;
use tracing::error;

pub async fn forward_request(
    client: &Client,
    method: Method,
    uri_path: &str,
    target_port: u16,
    headers: HeaderMap,
    payload: Option<Value>,
) -> Result<Response, (StatusCode, String)> {
    let target_url = format!("http://127.0.0.1:{}{}", target_port, uri_path);

    let mut req_builder = client.request(method.clone(), &target_url);

    // 헤더 복사 (host 헤더는 대상 서버에 맞게 제외)
    for (name, val) in headers.iter() {
        if name != "host" && name != "content-length" {
            if let Ok(h_name) = HeaderName::from_bytes(name.as_str().as_bytes()) {
                req_builder = req_builder.header(h_name, val.clone());
            }
        }
    }

    if let Some(json_body) = payload {
        req_builder = req_builder.json(&json_body);
    }

    let upstream_resp = req_builder
        .timeout(Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| {
            error!(target: "proxy", "Failed to connect to internal model backend: {}", e);
            (StatusCode::BAD_GATEWAY, format!("Upstream connection failed: {}", e))
        })?;

    let status = StatusCode::from_u16(upstream_resp.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    let mut response_builder = Response::builder().status(status);

    for (k, v) in upstream_resp.headers().iter() {
        if let Ok(name) = HeaderName::from_bytes(k.as_str().as_bytes()) {
            response_builder = response_builder.header(name, v.clone());
        }
    }

    // 비동기 바이트 스트림 중계 (SSE 스트리밍 완벽 지원)
    let stream = upstream_resp.bytes_stream();
    let body = Body::from_stream(stream);

    response_builder
        .body(body)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}
