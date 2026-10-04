#![cfg(not(target_arch = "wasm32"))]
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;
use storelib_rs::cancellation::CancellationToken;
use storelib_rs::{SfsClient, SfsConfig, SfsContentId, SfsError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn id() -> SfsContentId {
    SfsContentId {
        namespace: "default".into(),
        name: "app".into(),
        version: "exact.1".into(),
    }
}
fn client(server: &MockServer) -> SfsClient {
    let mut config = SfsConfig {
        base_url: format!("{}/namespaces/", server.uri()),
        ..Default::default()
    };
    config.http.max_retries = 0;
    config.http.timeout = Duration::from_secs(2);
    SfsClient::with_config(config).unwrap()
}
fn file(url: &str, bytes: &[u8]) -> Value {
    json!([{"FileId":"file", "Url":url, "SizeInBytes":bytes.len(),
        "Hashes":{"Sha256":format!("{:x}",Sha256::digest(bytes))}, "FileMoniker":"app.msix",
        "ApplicabilityDetails":{"Architectures":["amd64"],"PlatformApplicabilityForPackage":[]}}])
}
async fn metadata(server: &MockServer, value: Value) {
    Mock::given(method("POST"))
        .and(path("/namespaces/default/names/app/versions/exact.1/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(value))
        .mount(server)
        .await;
}
#[tokio::test]
async fn verified_download_returns_only_matching_bytes() {
    let server = MockServer::start().await;
    metadata(
        &server,
        file(
            &format!("{}/payload?token=secret", server.uri()),
            b"package",
        ),
    )
    .await;
    Mock::given(path("/payload"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"package"))
        .mount(&server)
        .await;
    assert_eq!(
        client(&server)
            .download_verified(&id(), "file", 32, None)
            .await
            .unwrap(),
        b"package"
    );
}
#[tokio::test]
async fn wrong_hash_rejects_complete_payload() {
    let server = MockServer::start().await;
    metadata(
        &server,
        file(&format!("{}/payload", server.uri()), b"package"),
    )
    .await;
    Mock::given(path("/payload"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"changed"))
        .mount(&server)
        .await;
    assert!(matches!(
        client(&server)
            .download_verified(&id(), "file", 32, None)
            .await,
        Err(SfsError::DownloadMismatch)
    ));
}
#[tokio::test]
async fn oversized_metadata_prevents_payload_request() {
    let server = MockServer::start().await;
    metadata(
        &server,
        file(&format!("{}/payload", server.uri()), b"package"),
    )
    .await;
    Mock::given(path("/payload"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    assert!(matches!(
        client(&server)
            .download_verified(&id(), "file", 2, None)
            .await,
        Err(SfsError::DownloadTooLarge { limit: 2 })
    ));
}
async fn refresh(server: &MockServer, changed: bool) {
    let initial = file(
        &format!("{}/expired?token=old-secret", server.uri()),
        b"package",
    );
    let renewed = file(
        &format!("{}/renewed?token=new-secret", server.uri()),
        if changed { b"changed" } else { b"package" },
    );
    let count = std::sync::atomic::AtomicUsize::new(0);
    Mock::given(path("/namespaces/default/names/app/versions/exact.1/files"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(
                if count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    initial.clone()
                } else {
                    renewed.clone()
                },
            )
        })
        .expect(2)
        .mount(server)
        .await;
    Mock::given(path("/expired"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(path("/renewed"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"package"))
        .expect(if changed { 0 } else { 1 })
        .mount(server)
        .await;
}
#[tokio::test]
async fn expired_url_refreshes_exact_file_and_verifies_payload() {
    let server = MockServer::start().await;
    refresh(&server, false).await;
    assert_eq!(
        client(&server)
            .download_verified(&id(), "file", 32, None)
            .await
            .unwrap(),
        b"package"
    );
}
#[tokio::test]
async fn changed_digest_during_refresh_rejects_replacement() {
    let server = MockServer::start().await;
    refresh(&server, true).await;
    assert!(matches!(
        client(&server)
            .download_verified(&id(), "file", 32, None)
            .await,
        Err(SfsError::FileChanged)
    ));
}
async fn raw_server(response: &'static [u8]) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let _ = socket.read(&mut request).await;
        socket.write_all(response).await.unwrap();
        socket.shutdown().await.unwrap();
    });
    format!("http://{addr}/payload?token=private-secret")
}
#[tokio::test]
async fn interrupted_payload_is_rejected_and_transport_error_redacts_url() {
    let server = MockServer::start().await;
    let url =
        raw_server(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npack").await;
    metadata(&server, file(&url, b"package")).await;
    let error = client(&server)
        .download_verified(&id(), "file", 32, None)
        .await
        .unwrap_err();
    assert!(matches!(error, SfsError::Transport(_)));
    assert!(!format!("{error:?} {error}").contains("private-secret"));
    if let SfsError::Transport(source) = error {
        assert!(source.url().is_none());
    }
}
#[tokio::test]
async fn chunked_payload_limit_is_enforced_without_content_length() {
    let server = MockServer::start().await;
    let url = raw_server(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n8\r\n12345678\r\n0\r\n\r\n").await;
    metadata(&server, file(&url, b"package")).await;
    assert!(matches!(
        client(&server)
            .download_verified(&id(), "file", 7, None)
            .await,
        Err(SfsError::DownloadTooLarge { limit: 7 })
    ));
}
#[tokio::test]
async fn cancellation_aborts_pending_payload_without_returning_bytes() {
    let server = MockServer::start().await;
    metadata(
        &server,
        file(&format!("{}/payload", server.uri()), b"package"),
    )
    .await;
    Mock::given(path("/payload"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(1))
                .set_body_bytes(b"package"),
        )
        .mount(&server)
        .await;
    let token = CancellationToken::new();
    let signal = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        signal.cancel();
    });
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_millis(500),
            client(&server).download_verified(&id(), "file", 32, Some(&token))
        )
        .await
        .unwrap(),
        Err(SfsError::Cancelled)
    ));
}

#[tokio::test]
async fn cancellation_interrupts_metadata_retry_backoff_and_stops_retries() {
    let server = MockServer::start().await;
    Mock::given(path("/namespaces/default/names/app/versions/exact.1/files"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let mut config = SfsConfig {
        base_url: format!("{}/namespaces/", server.uri()),
        ..Default::default()
    };
    config.http.max_retries = 3;
    config.http.initial_backoff = Duration::from_secs(1);
    config.http.max_backoff = Duration::from_secs(2);
    let client = SfsClient::with_config(config).unwrap();
    let token = CancellationToken::new();
    let signal = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        signal.cancel();
    });
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_millis(500),
            client.download_verified(&id(), "file", 32, Some(&token))
        )
        .await
        .unwrap(),
        Err(SfsError::Cancelled)
    ));
    tokio::time::sleep(Duration::from_millis(1100)).await;
    server.verify().await;
}

#[tokio::test]
async fn chunked_metadata_limit_is_enforced_without_content_length() {
    let url = raw_server(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n8\r\n12345678\r\n0\r\n\r\n").await;
    let endpoint = reqwest::Url::parse(&url).unwrap();
    let mut config = SfsConfig {
        base_url: format!("http://{}/namespaces/", endpoint.authority()),
        max_response_bytes: 7,
        ..Default::default()
    };
    config.http.max_retries = 0;
    assert!(matches!(
        SfsClient::with_config(config).unwrap().files(&id()).await,
        Err(SfsError::ResponseTooLarge { limit: 7 })
    ));
}

#[tokio::test]
async fn payload_timeout_rejects_delayed_response_and_redacts_location() {
    let server = MockServer::start().await;
    metadata(
        &server,
        file(
            &format!("{}/payload?token=private-secret", server.uri()),
            b"package",
        ),
    )
    .await;
    Mock::given(path("/payload"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(1))
                .set_body_bytes(b"package"),
        )
        .mount(&server)
        .await;
    let mut config = SfsConfig {
        base_url: format!("{}/namespaces/", server.uri()),
        ..Default::default()
    };
    config.http.timeout = Duration::from_millis(100);
    config.http.max_retries = 0;
    let error = SfsClient::with_config(config)
        .unwrap()
        .download_verified(&id(), "file", 32, None)
        .await
        .unwrap_err();
    assert!(matches!(error, SfsError::Transport(_)));
    assert!(!format!("{error:?}").contains("private-secret"));
}

#[tokio::test]
async fn complete_short_payload_is_rejected_by_expected_size() {
    let server = MockServer::start().await;
    metadata(
        &server,
        file(&format!("{}/payload", server.uri()), b"package"),
    )
    .await;
    Mock::given(path("/payload"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"pack"))
        .mount(&server)
        .await;
    assert!(matches!(
        client(&server)
            .download_verified(&id(), "file", 32, None)
            .await,
        Err(SfsError::DownloadMismatch)
    ));
}

#[tokio::test]
async fn repeatedly_expired_location_refreshes_only_once() {
    let server = MockServer::start().await;
    Mock::given(path("/namespaces/default/names/app/versions/exact.1/files"))
        .respond_with(ResponseTemplate::new(200).set_body_json(file(
            &format!("{}/expired?token=secret", server.uri()),
            b"package",
        )))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(path("/expired"))
        .respond_with(ResponseTemplate::new(403))
        .expect(2)
        .mount(&server)
        .await;
    assert!(matches!(
        client(&server)
            .download_verified(&id(), "file", 32, None)
            .await,
        Err(SfsError::Http { status: 403, .. })
    ));
}

#[tokio::test]
async fn pre_cancelled_download_sends_no_metadata_request() {
    let server = MockServer::start().await;
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        client(&server)
            .download_verified(&id(), "file", 32, Some(&token))
            .await,
        Err(SfsError::Cancelled)
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}
