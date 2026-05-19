//! Integration tests for ApiClient against a mock HTTP server.

use claude_o_meter::api::{ApiClient, FetchError};
use std::time::Duration;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn server() -> MockServer {
    MockServer::start().await
}

fn client(server: &MockServer) -> ApiClient {
    ApiClient::new_with_base("test-token".into(), server.uri()).expect("build client")
}

#[tokio::test]
async fn happy_path_200() {
    let s = server().await;
    let body = std::fs::read_to_string("tests/fixtures/usage_nominal.json").unwrap();
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .and(header("authorization", "Bearer test-token"))
        .and(header("anthropic-beta", "oauth-2025-04-20"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&s)
        .await;

    let resp = client(&s).fetch().await.unwrap();
    assert!(resp.five_hour.is_some());
}

#[tokio::test]
async fn unauthorized_401() {
    let s = server().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&s)
        .await;

    let err = client(&s).fetch().await.unwrap_err();
    assert!(matches!(err, FetchError::Unauthorized));
}

#[tokio::test]
async fn rate_limited_429_with_retry_after() {
    let s = server().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "120"))
        .mount(&s)
        .await;

    let err = client(&s).fetch().await.unwrap_err();
    match err {
        FetchError::RateLimited { retry_after } => {
            assert_eq!(retry_after, Some(Duration::from_secs(120)));
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[tokio::test]
async fn rate_limited_429_no_retry_after() {
    let s = server().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&s)
        .await;

    let err = client(&s).fetch().await.unwrap_err();
    assert!(matches!(err, FetchError::RateLimited { retry_after: None }));
}

#[tokio::test]
async fn server_error_5xx() {
    let s = server().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&s)
        .await;

    let err = client(&s).fetch().await.unwrap_err();
    assert!(matches!(err, FetchError::Server(503)));
}

#[tokio::test]
async fn malformed_body_is_decode_error() {
    let s = server().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&s)
        .await;

    let err = client(&s).fetch().await.unwrap_err();
    assert!(matches!(err, FetchError::Decode(_)));
}
