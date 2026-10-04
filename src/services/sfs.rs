//! Native Rust client for Microsoft's Simple File Solution Store app service.
//!
//! Protocol reference: microsoft/sfs-client at
//! `e7050f1505917c92cca01a43afedbd11d3975cf7` (MIT). This is an independent
//! implementation of its JSON request flow. File metadata and temporary URLs
//! are kept separate; this module does not install packages or issue licenses.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::future::Future;
use std::time::{Duration, SystemTime};

use futures_util::future::{select, Either};
use reqwest::{Method, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::display_catalog::ClientConfig;
use crate::cancellation::CancellationToken;
use crate::models::fe3::PackageInstance;
use crate::utilities::sleep::sleep;

const STORE_BASE: &str =
    "https://storeapps.api.cdp.microsoft.com/api/v2/contents/storeapps/namespaces/";

/// Service targeting keys supplied by the caller, not inferred OS policy.
pub type SfsTargetingAttributes = BTreeMap<String, String>;

/// Limits and HTTP policy for the SFS client.
#[derive(Debug, Clone)]
pub struct SfsConfig {
    /// HTTPS namespace-root endpoint. HTTP is allowed only on loopback for tests.
    pub base_url: String,
    /// Namespace for latest-version requests, normally `default`.
    pub namespace: String,
    /// Request timeout, retry count, and backoff policy.
    pub http: ClientConfig,
    /// Maximum bytes read for one metadata response, including decompressed bytes.
    pub max_response_bytes: usize,
    /// Maximum number of prerequisites accepted for one app.
    pub max_prerequisites: usize,
}

impl Default for SfsConfig {
    fn default() -> Self {
        let http = ClientConfig {
            initial_backoff: Duration::from_secs(15),
            max_backoff: Duration::from_secs(60),
            retry_on_status: vec![429, 500, 502, 503, 504],
            ..Default::default()
        };
        Self {
            base_url: STORE_BASE.into(),
            namespace: "default".into(),
            http,
            max_response_bytes: 8 * 1024 * 1024,
            max_prerequisites: 128,
        }
    }
}

/// Errors contain no response bodies or temporary download URLs.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SfsError {
    /// Invalid client configuration.
    #[error("Invalid SFS configuration: {0}")]
    InvalidConfig(&'static str),
    /// Invalid content identifier or targeting input.
    #[error("Invalid SFS input: {0}")]
    InvalidInput(&'static str),
    /// HTTP failure after any permitted retries.
    #[error("SFS returned HTTP {status}")]
    Http {
        /// HTTP status; 404 means the requested content was not available.
        status: u16,
        /// Server-provided retry delay, if valid.
        retry_after: Option<Duration>,
    },
    /// Transport failure, with any request URL removed from the source.
    #[error("SFS transport request failed")]
    Transport(#[source] reqwest::Error),
    /// Unexpected or incomplete metadata.
    #[error("Invalid SFS response: {0}")]
    InvalidResponse(&'static str),
    /// The service answered for a different namespace, name, or exact version.
    #[error("SFS response content identity does not match the request")]
    IdentityMismatch,
    /// The response exceeds the configured memory bound.
    #[error("SFS metadata response exceeds {limit} bytes")]
    ResponseTooLarge {
        /// Configured response limit.
        limit: usize,
    },
    /// The prerequisite list exceeds the configured bound.
    #[error("SFS prerequisite count exceeds {limit}")]
    TooManyPrerequisites {
        /// Configured prerequisite limit.
        limit: usize,
    },
    /// Cancelled request or backoff.
    #[error("SFS operation cancelled")]
    Cancelled,
    /// The requested file no longer exists, or refresh changed its expected bytes.
    #[error("SFS file identity changed during location refresh")]
    FileChanged,
    /// Download exceeds the caller's explicit memory bound.
    #[error("SFS download exceeds {limit} bytes")]
    DownloadTooLarge {
        /// Configured download limit.
        limit: usize,
    },
    /// Completed download does not match its expected size or SHA-256.
    #[error("SFS downloaded bytes do not match file metadata")]
    DownloadMismatch,
}

/// Exact SFS content identity; its version is opaque, not a package version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SfsContentId {
    /// Service namespace.
    pub namespace: String,
    /// Content name, commonly a Store `WuCategoryId` GUID.
    pub name: String,
    /// Exact version returned by the service.
    pub version: String,
}

impl SfsContentId {
    fn validate(&self) -> Result<(), SfsError> {
        for component in [&self.namespace, &self.name, &self.version] {
            validate_component(component)?;
        }
        if self.version == "latest" {
            return Err(SfsError::InvalidInput("an exact version is required"));
        }
        Ok(())
    }
}

/// Version metadata; app records contain an update ID and prerequisites.
#[derive(Debug, Clone, Serialize)]
pub struct SfsVersion {
    /// Exact content identity.
    pub content_id: SfsContentId,
    /// Present on app version records; absent on generic version records.
    pub update_id: Option<String>,
    /// Exact prerequisites declared by this version, without inferred closure.
    pub prerequisites: Vec<SfsContentId>,
}

/// Server applicability strings retained verbatim for caller-side selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SfsApplicability {
    /// For example `amd64`, `x86`, `arm64`, or `None`.
    pub architectures: Vec<String>,
    /// Device-family and minimum-version rules returned by SFS.
    pub platform_applicability_for_package: Vec<String>,
}

/// Durable file metadata, intentionally excluding its temporary location.
#[derive(Debug, Clone, Serialize)]
pub struct SfsFileMetadata {
    /// Stable service file identifier.
    pub file_id: String,
    /// Expected byte count.
    pub size_in_bytes: u64,
    /// Algorithm/value pairs preserved in their service encoding.
    pub hashes: BTreeMap<String, String>,
    /// Package moniker returned by the app delivery service.
    pub file_moniker: String,
    /// Architecture and platform applicability.
    pub applicability: SfsApplicability,
}

impl SfsFileMetadata {
    /// Expected SHA-256 in lowercase hex, if advertised by SFS.
    pub fn sha256_hex(&self) -> Option<String> {
        digest_hex(&self.hashes, "Sha256", 64)
    }

    /// Expected SHA-1 in lowercase hex, if advertised by SFS.
    pub fn sha1_hex(&self) -> Option<String> {
        digest_hex(&self.hashes, "Sha1", 40)
    }
}

/// Temporary download URL. Debug output is redacted; serialization is unsupported.
#[derive(Clone)]
pub struct SfsDownloadUrl(String);

impl SfsDownloadUrl {
    /// Explicitly expose the temporary location for a downloader.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SfsDownloadUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SfsDownloadUrl([REDACTED])")
    }
}

/// App file metadata and its transient download location.
#[derive(Debug, Clone)]
pub struct SfsFile {
    /// Serializable metadata.
    pub metadata: SfsFileMetadata,
    /// Transient URL, never serialized with metadata.
    pub download_url: SfsDownloadUrl,
}

/// Files for one exact declared prerequisite.
#[derive(Debug, Clone)]
pub struct SfsPrerequisite {
    /// Exact identity requested, including its namespace.
    pub content_id: SfsContentId,
    /// App file candidates returned for that identity.
    pub files: Vec<SfsFile>,
}

/// Main app and its declared prerequisites. This is metadata, not an install plan.
#[derive(Debug, Clone)]
pub struct SfsAppDownloadInfo {
    /// Main app version metadata.
    pub version: SfsVersion,
    /// Main app file candidates.
    pub files: Vec<SfsFile>,
    /// Resolved files for the exact prerequisites declared by SFS.
    pub prerequisites: Vec<SfsPrerequisite>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WireVersion {
    content_id: SfsContentId,
    update_id: Option<String>,
    prerequisites: Option<Vec<SfsContentId>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WireFile {
    file_id: String,
    url: String,
    size_in_bytes: u64,
    hashes: BTreeMap<String, String>,
    file_moniker: String,
    applicability_details: SfsApplicability,
}

/// Cloneable native async SFS client. Supply a Reqwest client for custom TLS/proxies.
#[derive(Clone)]
pub struct SfsClient {
    client: reqwest::Client,
    base: Url,
    config: SfsConfig,
}

impl SfsClient {
    /// Download one exact file into bounded memory and verify its size and SHA-256.
    ///
    /// Refreshes exact file locations once after HTTP 401/403. The refreshed
    /// file must retain its file ID, size, and SHA-256. Other HTTP failures and
    /// interrupted transfers are returned without accepting partial bytes.
    /// The default client refuses redirects; custom clients must do so too. This method does
    /// not write files; use a modest explicit `max_bytes` for package inspection.
    pub async fn download_verified(
        &self,
        identity: &SfsContentId,
        file_id: &str,
        max_bytes: usize,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<u8>, SfsError> {
        cancellable(
            async {
                if max_bytes == 0 {
                    return Err(SfsError::InvalidInput("download limit must be positive"));
                }
                let mut file = self
                    .files(identity)
                    .await?
                    .into_iter()
                    .find(|file| file.metadata.file_id == file_id)
                    .ok_or(SfsError::FileChanged)?;
                let expected_size = file.metadata.size_in_bytes;
                let expected_hash = file.metadata.sha256_hex().ok_or(SfsError::InvalidResponse(
                    "SHA-256 required for verified download",
                ))?;
                if expected_size > max_bytes as u64 {
                    return Err(SfsError::DownloadTooLarge { limit: max_bytes });
                }
                for attempt in 0..=1 {
                    let mut response = self
                        .client
                        .get(file.download_url.as_str())
                        .timeout(self.config.http.timeout)
                        .header(reqwest::header::ACCEPT_ENCODING, "identity")
                        .send()
                        .await
                        .map_err(transport_error)?;
                    // Reject an unexpected final location in addition to the default no-redirect policy.
                    if response.url().as_str() != file.download_url.as_str() {
                        return Err(SfsError::InvalidResponse("download redirected"));
                    }
                    if matches!(response.status().as_u16(), 401 | 403) && attempt == 0 {
                        drop(response);
                        file = self
                            .files(identity)
                            .await?
                            .into_iter()
                            .find(|file| file.metadata.file_id == file_id)
                            .ok_or(SfsError::FileChanged)?;
                        if file.metadata.size_in_bytes != expected_size
                            || file.metadata.sha256_hex().as_deref() != Some(&expected_hash)
                        {
                            return Err(SfsError::FileChanged);
                        }
                        continue;
                    }
                    if !response.status().is_success() {
                        return Err(SfsError::Http {
                            status: response.status().as_u16(),
                            retry_after: None,
                        });
                    }
                    if response
                        .content_length()
                        .is_some_and(|size| size > max_bytes as u64)
                    {
                        return Err(SfsError::DownloadTooLarge { limit: max_bytes });
                    }
                    let mut bytes = Vec::new();
                    let mut digest = Sha256::new();
                    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
                        if chunk.len() > max_bytes.saturating_sub(bytes.len()) {
                            return Err(SfsError::DownloadTooLarge { limit: max_bytes });
                        }
                        digest.update(&chunk);
                        bytes.extend_from_slice(&chunk);
                    }
                    if bytes.len() as u64 != expected_size
                        || format!("{:x}", digest.finalize()) != expected_hash
                    {
                        return Err(SfsError::DownloadMismatch);
                    }
                    return Ok(bytes);
                }
                Err(SfsError::FileChanged)
            },
            cancel,
        )
        .await
    }

    /// Construct a client for Store apps using the default policy.
    pub fn new() -> Result<Self, SfsError> {
        Self::with_config(SfsConfig::default())
    }

    /// Construct with bounded HTTP policy and endpoint configuration.
    pub fn with_config(config: SfsConfig) -> Result<Self, SfsError> {
        let cert = reqwest::Certificate::from_pem(include_bytes!("../../data/ms_root_ca_2011.pem"))
            .map_err(|_| SfsError::InvalidConfig("invalid bundled Microsoft root"))?;
        let client = reqwest::Client::builder()
            .user_agent(&config.http.user_agent)
            .timeout(config.http.timeout)
            .add_root_certificate(cert)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(transport_error)?;
        Self::with_client(client, config)
    }

    /// Supply a configured client. It must validate TLS and avoid unsafe redirects.
    /// Per-request timeouts and response limits still apply.
    pub fn with_client(client: reqwest::Client, config: SfsConfig) -> Result<Self, SfsError> {
        validate_component(&config.namespace)?;
        if config.max_response_bytes == 0
            || config.http.max_retries > 10
            || config.http.timeout.is_zero()
        {
            return Err(SfsError::InvalidConfig(
                "invalid response limit, timeout, or retry count",
            ));
        }
        let mut base = Url::parse(&config.base_url)
            .map_err(|_| SfsError::InvalidConfig("invalid namespace-root URL"))?;
        let loopback = matches!(base.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if !(base.scheme() == "https" || (base.scheme() == "http" && loopback))
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || base.cannot_be_a_base()
        {
            return Err(SfsError::InvalidConfig(
                "namespace-root URL must be HTTPS without credentials, query, or fragment",
            ));
        }
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        Ok(Self {
            client,
            base,
            config,
        })
    }

    /// Select the latest app version for a category and caller-supplied attributes.
    pub async fn latest_version(
        &self,
        name: &str,
        attributes: &SfsTargetingAttributes,
    ) -> Result<SfsVersion, SfsError> {
        self.latest_version_with_cancel(name, attributes, None)
            .await
    }

    /// Cancellable latest-version lookup.
    pub async fn latest_version_with_cancel(
        &self,
        name: &str,
        attributes: &SfsTargetingAttributes,
        cancel: Option<&CancellationToken>,
    ) -> Result<SfsVersion, SfsError> {
        cancellable(
            async {
                let mut url = self.version_url(&self.config.namespace, name, "latest")?;
                url.set_query(Some("action=select"));
                let wire: WireVersion = self
                    .request_json(
                        Method::POST,
                        url,
                        Some(serde_json::json!({"TargetingAttributes": attributes})),
                    )
                    .await?;
                let version = self.validate_version(wire, &self.config.namespace, name, None)?;
                if version.update_id.is_none() {
                    return Err(SfsError::InvalidResponse(
                        "latest content is not an app version",
                    ));
                }
                Ok(version)
            },
            cancel,
        )
        .await
    }

    /// Read metadata for an exact content identity; never substitute latest.
    pub async fn version(&self, identity: &SfsContentId) -> Result<SfsVersion, SfsError> {
        identity.validate()?;
        let url = self.version_url(&identity.namespace, &identity.name, &identity.version)?;
        let wire: WireVersion = self.request_json(Method::GET, url, None).await?;
        self.validate_version(
            wire,
            &identity.namespace,
            &identity.name,
            Some(&identity.version),
        )
    }

    /// Generate temporary file locations for an exact content identity.
    pub async fn files(&self, identity: &SfsContentId) -> Result<Vec<SfsFile>, SfsError> {
        self.files_with_cancel(identity, None).await
    }

    /// Cancellable exact file-location generation.
    pub async fn files_with_cancel(
        &self,
        identity: &SfsContentId,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<SfsFile>, SfsError> {
        cancellable(
            async {
                identity.validate()?;
                let mut url =
                    self.version_url(&identity.namespace, &identity.name, &identity.version)?;
                url.path_segments_mut()
                    .map_err(|_| SfsError::InvalidConfig("invalid base path"))?
                    .push("files");
                url.set_query(Some("action=GenerateDownloadInfo"));
                let files: Vec<WireFile> = self.request_json(Method::POST, url, None).await?;
                if files.is_empty() {
                    return Err(SfsError::InvalidResponse("no app files returned"));
                }
                let mut ids = HashSet::new();
                files
                    .into_iter()
                    .map(|file| {
                        if !ids.insert(file.file_id.clone()) {
                            return Err(SfsError::InvalidResponse("duplicate file identity"));
                        }
                        validate_file(file)
                    })
                    .collect()
            },
            cancel,
        )
        .await
    }

    /// Resolve the latest app and files for each declared exact prerequisite.
    /// This does not recursively infer dependencies or select a Windows target.
    pub async fn get_latest_app_download_info(
        &self,
        name: &str,
        attributes: &SfsTargetingAttributes,
    ) -> Result<SfsAppDownloadInfo, SfsError> {
        self.get_latest_app_download_info_with_cancel(name, attributes, None)
            .await
    }

    /// Cancellation drops the entire metadata/prerequisite request sequence.
    pub async fn get_latest_app_download_info_with_cancel(
        &self,
        name: &str,
        attributes: &SfsTargetingAttributes,
        cancel: Option<&CancellationToken>,
    ) -> Result<SfsAppDownloadInfo, SfsError> {
        cancellable(
            async {
                let version = self.latest_version(name, attributes).await?;
                let files = self.files(&version.content_id).await?;
                let mut prerequisites = Vec::new();
                let mut seen = HashSet::new();
                for identity in &version.prerequisites {
                    if identity == &version.content_id || !seen.insert(identity.clone()) {
                        continue;
                    }
                    prerequisites.push(SfsPrerequisite {
                        content_id: identity.clone(),
                        files: self.files(identity).await?,
                    });
                }
                Ok(SfsAppDownloadInfo {
                    version,
                    files,
                    prerequisites,
                })
            },
            cancel,
        )
        .await
    }

    fn version_url(&self, namespace: &str, name: &str, version: &str) -> Result<Url, SfsError> {
        for component in [namespace, name, version] {
            validate_component(component)?;
        }
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| SfsError::InvalidConfig("invalid base path"))?
            .pop_if_empty()
            .push(namespace)
            .push("names")
            .push(name)
            .push("versions")
            .push(version);
        Ok(url)
    }

    fn validate_version(
        &self,
        wire: WireVersion,
        namespace: &str,
        name: &str,
        exact_version: Option<&str>,
    ) -> Result<SfsVersion, SfsError> {
        wire.content_id
            .validate()
            .map_err(|_| SfsError::InvalidResponse("invalid content identity"))?;
        if wire.content_id.namespace != namespace
            || wire.content_id.name != name
            || exact_version.is_some_and(|v| v != wire.content_id.version)
        {
            return Err(SfsError::IdentityMismatch);
        }
        if wire.update_id.as_deref().is_some_and(str::is_empty) {
            return Err(SfsError::InvalidResponse("empty update identity"));
        }
        if wire.update_id.is_some() && wire.prerequisites.is_none() {
            return Err(SfsError::InvalidResponse(
                "app version is missing prerequisites",
            ));
        }
        let prerequisites = wire.prerequisites.unwrap_or_default();
        if prerequisites.len() > self.config.max_prerequisites {
            return Err(SfsError::TooManyPrerequisites {
                limit: self.config.max_prerequisites,
            });
        }
        for prerequisite in &prerequisites {
            prerequisite
                .validate()
                .map_err(|_| SfsError::InvalidResponse("invalid prerequisite identity"))?;
        }
        Ok(SfsVersion {
            content_id: wire.content_id,
            update_id: wire.update_id,
            prerequisites,
        })
    }

    async fn request_json<T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        body: Option<serde_json::Value>,
    ) -> Result<T, SfsError> {
        for attempt in 0..=self.config.http.max_retries {
            let mut request = self
                .client
                .request(method.clone(), url.clone())
                .timeout(self.config.http.timeout)
                .header("Content-Type", "application/json");
            if let Some(body) = &body {
                request = request.json(body);
            }
            let result = request.send().await;
            let mut response = match result {
                Ok(response) => response,
                Err(error)
                    if attempt < self.config.http.max_retries
                        && (error.is_connect() || error.is_timeout()) =>
                {
                    sleep(self.config.http.backoff_for(attempt)).await;
                    continue;
                }
                Err(error) => return Err(transport_error(error)),
            };
            let retry_after = response
                .headers()
                .get("Retry-After")
                .and_then(|h| h.to_str().ok())
                .and_then(parse_retry_after);
            if !response.status().is_success() {
                let status = response.status().as_u16();
                if attempt < self.config.http.max_retries
                    && self.config.http.retry_on_status.contains(&status)
                {
                    let delay =
                        retry_after.unwrap_or_else(|| self.config.http.backoff_for(attempt));
                    // Do not retry earlier than a server's delay merely to obey a local cap.
                    if delay > self.config.http.max_backoff {
                        return Err(SfsError::Http {
                            status,
                            retry_after,
                        });
                    }
                    drop(response);
                    sleep(delay).await;
                    continue;
                }
                return Err(SfsError::Http {
                    status,
                    retry_after,
                });
            }
            let limit = self.config.max_response_bytes;
            if response
                .content_length()
                .is_some_and(|length| length > limit as u64)
            {
                return Err(SfsError::ResponseTooLarge { limit });
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
                if chunk.len() > limit.saturating_sub(bytes.len()) {
                    return Err(SfsError::ResponseTooLarge { limit });
                }
                bytes.extend_from_slice(&chunk);
            }
            return serde_json::from_slice(&bytes)
                .map_err(|_| SfsError::InvalidResponse("invalid JSON metadata"));
        }
        Err(SfsError::InvalidConfig("invalid retry policy"))
    }
}

fn transport_error(error: reqwest::Error) -> SfsError {
    SfsError::Transport(error.without_url())
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
        .or_else(|| {
            httpdate::parse_http_date(value)
                .ok()
                .map(|date| date.duration_since(SystemTime::now()).unwrap_or_default())
        })
}

fn validate_component(value: &str) -> Result<(), SfsError> {
    if value.is_empty()
        || value.len() > 1024
        || matches!(value, "." | "..")
        || value.chars().any(char::is_control)
    {
        return Err(SfsError::InvalidInput(
            "empty or invalid content identity component",
        ));
    }
    Ok(())
}

fn digest_hex(hashes: &BTreeMap<String, String>, algorithm: &str, length: usize) -> Option<String> {
    hashes
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(algorithm))
        .and_then(|(_, value)| PackageInstance::digest_to_hex(value))
        .filter(|digest| digest.len() == length)
}

fn validate_file(file: WireFile) -> Result<SfsFile, SfsError> {
    if file.file_id.is_empty() || file.file_moniker.is_empty() || file.hashes.is_empty() {
        return Err(SfsError::InvalidResponse(
            "missing app file identity or hashes",
        ));
    }
    let mut algorithms = HashSet::new();
    for (key, value) in &file.hashes {
        if !algorithms.insert(key.to_ascii_lowercase()) {
            return Err(SfsError::InvalidResponse("duplicate hash algorithm"));
        }
        let length = if key.eq_ignore_ascii_case("Sha256") {
            Some(64)
        } else if key.eq_ignore_ascii_case("Sha1") {
            Some(40)
        } else {
            None
        };
        if let Some(length) = length {
            if PackageInstance::digest_to_hex(value).is_none_or(|digest| digest.len() != length) {
                return Err(SfsError::InvalidResponse("invalid file digest"));
            }
        }
    }
    let url =
        Url::parse(&file.url).map_err(|_| SfsError::InvalidResponse("invalid download URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(SfsError::InvalidResponse("invalid download URL"));
    }
    Ok(SfsFile {
        metadata: SfsFileMetadata {
            file_id: file.file_id,
            size_in_bytes: file.size_in_bytes,
            hashes: file.hashes,
            file_moniker: file.file_moniker,
            applicability: file.applicability_details,
        },
        download_url: SfsDownloadUrl(file.url),
    })
}

async fn cancellable<T>(
    operation: impl Future<Output = Result<T, SfsError>>,
    cancel: Option<&CancellationToken>,
) -> Result<T, SfsError> {
    if let Some(token) = cancel {
        if token.is_cancelled() {
            return Err(SfsError::Cancelled);
        }
        let operation = std::pin::pin!(operation);
        let cancelled = std::pin::pin!(token.cancelled());
        match select(operation, cancelled).await {
            Either::Left((result, _)) => result,
            Either::Right(_) => Err(SfsError::Cancelled),
        }
    } else {
        operation.await
    }
}
