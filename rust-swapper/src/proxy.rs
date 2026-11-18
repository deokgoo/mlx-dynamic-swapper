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
    raw_body: axum::body::Bytes,
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

    // JSON 은 파이프라인을 거친 값을, 그 밖의 본문(multipart·form·바이너리 등)은 받은 그대로 넘깁니다.
    if let Some(json_body) = payload {
        req_builder = req_builder.json(&json_body);
    } else if !raw_body.is_empty() {
        req_builder = req_builder.body(raw_body);
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;
    use axum::Router;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn forwards_non_json_body_unchanged() {
        // 받은 본문을 그대로 돌려주는 업스트림
        let app = Router::new().route("/echo", post(|body: axum::body::Bytes| async move { body }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let raw = axum::body::Bytes::from_static(b"--boundary\r\nnot json\r\n--boundary--\r\n");
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "multipart/form-data; boundary=boundary".parse().unwrap());

        let resp = forward_request(&Client::new(), Method::POST, "/echo", port, headers, None, raw.clone())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let echoed = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(echoed, raw);
    }
}
