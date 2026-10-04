#![cfg(not(target_arch = "wasm32"))]

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;
use storelib_rs::cancellation::CancellationToken;
use storelib_rs::{SfsClient, SfsConfig, SfsContentId, SfsError};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn identity() -> SfsContentId {
    SfsContentId {
        namespace: "default".into(),
        name: "app".into(),
        version: "opaque.1".into(),
    }
}
fn version() -> Value {
    json!({"ContentId":identity(), "UpdateId":"update", "Prerequisites":[]})
}
fn file() -> Value {
    json!({"FileId":"file", "Url":"https://example.invalid/package?token=secret",
        "SizeInBytes":42,"Hashes":{"Sha256":"00".repeat(32)},"FileMoniker":"package",
        "ApplicabilityDetails":{"Architectures":["amd64"],"PlatformApplicabilityForPackage":["Windows.Desktop"]}})
}
fn client(server: &MockServer) -> SfsClient {
    configured(server, |_| {})
}
fn configured(server: &MockServer, change: impl FnOnce(&mut SfsConfig)) -> SfsClient {
    let mut config = SfsConfig {
        base_url: format!("{}/namespaces/", server.uri()),
        ..Default::default()
    };
    config.http.max_retries = 1;
    config.http.initial_backoff = Duration::from_millis(1);
    config.http.max_backoff = Duration::from_millis(20);
    change(&mut config);
    SfsClient::with_config(config).unwrap()
}
async fn latest_mock(server: &MockServer, value: Value) {
    Mock::given(method("POST"))
        .and(path("/namespaces/default/names/app/versions/latest"))
        .and(query_param("action", "select"))
        .and(body_json(json!({"TargetingAttributes":{}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(value))
        .expect(1)
        .mount(server)
        .await;
}
async fn files_mock(server: &MockServer, namespace: &str, name: &str, version: &str, value: Value) {
    Mock::given(method("POST"))
        .and(path(format!(
            "/namespaces/{namespace}/names/{name}/versions/{version}/files"
        )))
        .and(query_param("action", "GenerateDownloadInfo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(value))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn resolves_exact_prerequisite_namespace_and_deduplicates() {
    let server = MockServer::start().await;
    let mut app = version();
    let prerequisite = json!({"Namespace":"frameworks","Name":"dependency","Version":"exact.9"});
    app["Prerequisites"] = json!([prerequisite, prerequisite]);
    latest_mock(&server, app).await;
    files_mock(&server, "default", "app", "opaque.1", json!([file()])).await;
    files_mock(
        &server,
        "frameworks",
        "dependency",
        "exact.9",
        json!([file()]),
    )
    .await;
    let info = client(&server)
        .get_latest_app_download_info("app", &BTreeMap::new())
        .await
        .unwrap();
    assert_eq!(info.prerequisites.len(), 1);
    assert_eq!(info.prerequisites[0].content_id.namespace, "frameworks");
    assert_eq!(info.files[0].metadata.sha256_hex(), Some("00".repeat(32)));
    assert!(!format!("{info:?}").contains("token=secret"));
    assert!(!serde_json::to_string(&info.files[0].metadata)
        .unwrap()
        .contains("Url"));
    assert!(info.files[0].download_url.as_str().contains("token=secret"));
}

#[tokio::test]
async fn exact_version_rejects_substituted_identity() {
    let server = MockServer::start().await;
    let mut value = version();
    value["ContentId"]["Version"] = json!("other");
    Mock::given(method("GET"))
        .and(path("/namespaces/default/names/app/versions/opaque.1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(value))
        .mount(&server)
        .await;
    assert!(matches!(
        client(&server).version(&identity()).await,
        Err(SfsError::IdentityMismatch)
    ));
}

#[tokio::test]
async fn exact_generic_version_is_supported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ContentId":identity()})))
        .mount(&server)
        .await;
    assert!(client(&server)
        .version(&identity())
        .await
        .unwrap()
        .update_id
        .is_none());
}

#[tokio::test]
async fn latest_requires_app_prerequisite_metadata() {
    let server = MockServer::start().await;
    let mut value = version();
    value.as_object_mut().unwrap().remove("Prerequisites");
    latest_mock(&server, value).await;
    assert!(matches!(
        client(&server)
            .latest_version("app", &BTreeMap::new())
            .await,
        Err(SfsError::InvalidResponse(_))
    ));
}

#[tokio::test]
async fn malformed_file_metadata_is_rejected() {
    for value in [
        json!([]),
        json!([file(), file()]),
        {
            let mut value = file();
            value["Hashes"]["Sha256"] = json!("abcd");
            json!([value])
        },
        {
            let mut value = file();
            value["Url"] = json!("file:///tmp/package");
            json!([value])
        },
    ] {
        let server = MockServer::start().await;
        files_mock(&server, "default", "app", "opaque.1", value).await;
        assert!(matches!(
            client(&server).files(&identity()).await,
            Err(SfsError::InvalidResponse(_))
        ));
    }
}

#[tokio::test]
async fn response_and_prerequisite_limits_are_enforced() {
    let server = MockServer::start().await;
    latest_mock(&server, version()).await;
    let bounded = configured(&server, |c| c.max_response_bytes = 10);
    assert!(matches!(
        bounded.latest_version("app", &BTreeMap::new()).await,
        Err(SfsError::ResponseTooLarge { limit: 10 })
    ));
    let server = MockServer::start().await;
    let mut value = version();
    value["Prerequisites"] = json!([identity()]);
    latest_mock(&server, value).await;
    let bounded = configured(&server, |c| c.max_prerequisites = 0);
    assert!(matches!(
        bounded.latest_version("app", &BTreeMap::new()).await,
        Err(SfsError::TooManyPrerequisites { limit: 0 })
    ));
}

#[tokio::test]
async fn retry_budget_is_bounded_and_long_server_delay_is_preserved() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .expect(2)
        .mount(&server)
        .await;
    assert!(matches!(
        client(&server)
            .latest_version("app", &BTreeMap::new())
            .await,
        Err(SfsError::Http { status: 503, .. })
    ));
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "120"))
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        matches!(client(&server).latest_version("app",&BTreeMap::new()).await,Err(SfsError::Http{status:429,retry_after:Some(delay)}) if delay==Duration::from_secs(120))
    );
}

#[tokio::test]
async fn cancellation_interrupts_in_flight_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(version())
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    let token = CancellationToken::new();
    let cloned = token.clone();
    let cancel = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        cloned.cancel();
    });
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        client(&server).latest_version_with_cancel("app", &BTreeMap::new(), Some(&token)),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(SfsError::Cancelled)));
    cancel.await.unwrap();
}

#[tokio::test]
async fn pre_cancelled_operation_sends_no_request() {
    let server = MockServer::start().await;
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        client(&server)
            .files_with_cancel(&identity(), Some(&token))
            .await,
        Err(SfsError::Cancelled)
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[test]
fn unsafe_endpoints_are_rejected() {
    for base_url in [
        "http://example.com/",
        "https://user:pass@example.com/",
        "https://example.com/?token=secret",
    ] {
        assert!(matches!(
            SfsClient::with_config(SfsConfig {
                base_url: base_url.into(),
                ..Default::default()
            }),
            Err(SfsError::InvalidConfig(_))
        ));
    }
}

#[tokio::test]
async fn latest_cannot_be_used_as_an_exact_version() {
    let server = MockServer::start().await;
    let mut id = identity();
    id.version = "latest".into();
    assert!(matches!(
        client(&server).version(&id).await,
        Err(SfsError::InvalidInput(_))
    ));
    assert!(matches!(
        client(&server).files(&id).await,
        Err(SfsError::InvalidInput(_))
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn prerequisite_failure_does_not_return_partial_success() {
    let server = MockServer::start().await;
    let mut value = version();
    value["Prerequisites"] = json!([{"Namespace":"frameworks","Name":"missing","Version":"1"}]);
    latest_mock(&server, value).await;
    files_mock(&server, "default", "app", "opaque.1", json!([file()])).await;
    assert!(matches!(
        client(&server)
            .get_latest_app_download_info("app", &BTreeMap::new())
            .await,
        Err(SfsError::Http { status: 404, .. })
    ));
}

#[tokio::test]
#[ignore = "live Microsoft metadata and bounded payload download"]
async fn live_terminal_payload_matches_advertised_size_and_sha256() {
    use sha2::{Digest, Sha256};
    use storelib_rs::{DisplayCatalogHandler, IdentifierType};
    let mut catalog = DisplayCatalogHandler::production();
    catalog
        .query_dcat("9N0DX20HK701", IdentifierType::ProductId, None)
        .await
        .unwrap();
    let info = catalog.get_sfs_download_info().await.unwrap();
    let file = info
        .files
        .iter()
        .min_by_key(|file| file.metadata.size_in_bytes)
        .unwrap();
    let limit = 64 * 1024 * 1024;
    assert!(file.metadata.size_in_bytes <= limit);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap();
    let response = client.get(file.download_url.as_str()).send().await;
    let mut response = response.map_err(|error| error.without_url()).unwrap();
    assert!(response.status().is_success());
    let mut digest = Sha256::new();
    let mut size = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| error.without_url())
        .unwrap()
    {
        size += chunk.len() as u64;
        assert!(size <= limit);
        digest.update(&chunk);
    }
    assert_eq!(size, file.metadata.size_in_bytes);
    assert_eq!(
        format!("{:x}", digest.finalize()),
        file.metadata.sha256_hex().unwrap()
    );
}
