use std::future::Future;
use std::time::Duration;

use futures_util::future::{select, Either};

use crate::cancellation::CancellationToken;
use crate::error::StoreError;
use crate::models::catalog::{
    Availability, DisplayCatalogModel, FrameworkDependency, Image, Package, PlatformDependency,
    Price, Product, ProductLocalizedProperty, Sku,
};
use crate::models::enums::{DCatEndpoint, DeviceFamily, DisplayCatalogResult, IdentifierType};
use crate::models::fe3::PackageInstance;
use crate::models::locale::Locale;
use crate::models::search::DCatSearch;
use crate::services::fe3::FE3Handler;
use crate::utilities::helpers::{create_dcat_uri, endpoint_to_search_url};
use crate::utilities::sleep::sleep;
use log::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// HTTP client config
// ---------------------------------------------------------------------------

/// Tunable HTTP-client parameters shared by [`DisplayCatalogHandler`] and the
/// FE3 SOAP calls it triggers.
///
/// Construct with [`Default`] for the recommended values, or build one
/// explicitly:
///
/// ```
/// use std::time::Duration;
/// use storelib_rs::ClientConfig;
///
/// let cfg = ClientConfig {
///     timeout: Duration::from_secs(60),
///     max_retries: 5,
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Per-request timeout. The whole request (incl. body read) must finish
    /// inside this window or it fails with [`StoreError::TimedOut`].
    /// Default: 30 s.
    pub timeout: Duration,
    /// Maximum retry attempts on transient failures
    /// (connection errors, timeouts, statuses in [`Self::retry_on_status`]).
    /// `0` disables retries. Default: `3`.
    pub max_retries: u32,
    /// Initial backoff between attempts; doubled each retry up to
    /// [`Self::max_backoff`]. Default: 500 ms.
    pub initial_backoff: Duration,
    /// Upper cap on per-attempt backoff. Default: 5 s.
    pub max_backoff: Duration,
    /// HTTP status codes that trigger a retry. Default:
    /// `[408, 429, 502, 503, 504]`.
    pub retry_on_status: Vec<u16>,
    /// User-Agent header. Default: `"StoreLib"`.
    pub user_agent: String,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_retries: 3,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(5),
            retry_on_status: vec![408, 429, 502, 503, 504],
            user_agent: "StoreLib".into(),
        }
    }
}

impl ClientConfig {
    pub(crate) fn backoff_for(&self, attempt: u32) -> Duration {
        // attempt 0 → initial; attempt 1 → 2× initial; attempt N → 2^N × initial, capped.
        let factor: u64 = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
        let nanos = self
            .initial_backoff
            .as_nanos()
            .saturating_mul(factor as u128);
        let dur = Duration::from_nanos(nanos.min(u64::MAX as u128) as u64);
        std::cmp::min(dur, self.max_backoff)
    }
}

/// Race `op` against an optional cancellation token. Returns
/// [`StoreError::Cancelled`] if the token is cancelled before `op` completes.
pub(crate) async fn race_cancel<F, T>(
    op: F,
    cancel: Option<&CancellationToken>,
) -> Result<T, StoreError>
where
    F: Future<Output = Result<T, StoreError>>,
{
    let op = std::pin::pin!(op);
    match cancel {
        Some(tok) => {
            let cancel_fut = tok.cancelled();
            let cancel_fut = std::pin::pin!(cancel_fut);
            match select(op, cancel_fut).await {
                Either::Left((res, _)) => res,
                Either::Right(_) => Err(StoreError::Cancelled),
            }
        }
        None => op.await,
    }
}

/// Send a reqwest request with exponential-backoff retries, optional
/// cancellation, and progress events on each retry boundary.
///
/// `make_req` is called once per attempt — it must rebuild the
/// [`reqwest::RequestBuilder`] each time because `RequestBuilder` isn't
/// cloneable. `on_progress` receives `("retry.wait", …)` before each backoff
/// sleep and `("retry.attempt", …)` after the sleep, just before the next
/// send.
///
/// Returns the [`reqwest::Response`] on success or on the final attempt
/// (caller inspects status). Returns [`StoreError::Cancelled`] if the token
/// fires at any point (including mid-backoff). Returns [`StoreError::TimedOut`]
/// or [`StoreError::Http`] when retries are exhausted on a transport error.
pub(crate) async fn send_with_retry<F, P>(
    make_req: F,
    cfg: &ClientConfig,
    cancel: Option<&CancellationToken>,
    on_progress: P,
) -> Result<reqwest::Response, StoreError>
where
    F: Fn() -> reqwest::RequestBuilder,
    P: Fn(&'static str, String),
{
    let total_attempts = cfg.max_retries.saturating_add(1);
    let mut last_err: Option<StoreError> = None;

    for attempt in 0..total_attempts {
        if attempt > 0 {
            let backoff = cfg.backoff_for(attempt - 1);
            on_progress(
                "retry.wait",
                format!(
                    "waiting {}ms before attempt {}/{}",
                    backoff.as_millis(),
                    attempt + 1,
                    total_attempts,
                ),
            );
            // Cancellation-aware sleep. Returns Cancelled if the token fires.
            race_cancel(
                async {
                    sleep(backoff).await;
                    Ok::<(), StoreError>(())
                },
                cancel,
            )
            .await?;
            on_progress(
                "retry.attempt",
                format!("attempt {}/{}", attempt + 1, total_attempts),
            );
        }

        let send_fut = make_req().send();
        let result = race_cancel(
            async {
                send_fut.await.map_err(|e| {
                    if e.is_timeout() {
                        StoreError::TimedOut
                    } else {
                        StoreError::Http(e)
                    }
                })
            },
            cancel,
        )
        .await;

        match result {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let retryable = cfg.retry_on_status.contains(&status);
                let attempts_left = attempt + 1 < total_attempts;
                if retryable && attempts_left {
                    // Drop the response and try again.
                    continue;
                }
                return Ok(resp);
            }
            Err(StoreError::Cancelled) => return Err(StoreError::Cancelled),
            Err(e) => {
                if attempt + 1 < total_attempts {
                    last_err = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }

    Err(last_err.unwrap_or_else(|| StoreError::Other("retries exhausted".into())))
}

// ---------------------------------------------------------------------------
// Progress reporting
// ---------------------------------------------------------------------------

/// A real-time progress update emitted during long-running operations
/// (`query_dcat`, `get_packages_for_product`, `search_dcat`).
///
/// `stage` is a stable identifier (e.g. `"fe3.syncUpdates"`); `message`
/// is human-readable detail. `current`/`total` are populated for stages
/// that report `"N of M"` counters.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    /// @ts-type ProgressStage
    pub stage: &'static str,
    pub message: String,
    pub current: Option<u32>,
    pub total: Option<u32>,
}

/// Callback type for [`ProgressEmitter::set`].
///
/// On native targets the callback must be `Send + Sync` so the handler
/// stays multi-thread-friendly. On WASM the bound is relaxed to plain
/// `'static` because `js_sys::Function` is `!Send`.
#[cfg(not(target_arch = "wasm32"))]
pub type ProgressCallback = Box<dyn Fn(ProgressEvent) + Send + Sync + 'static>;
#[cfg(target_arch = "wasm32")]
pub type ProgressCallback = Box<dyn Fn(ProgressEvent) + 'static>;

/// Optional [`ProgressCallback`] plus its `emit` plumbing — embedded as a
/// public field on every handler that publishes progress events, so the
/// `set` / `clear` / `emit` API lives in one place.
#[derive(Default)]
pub struct ProgressEmitter {
    callback: Option<ProgressCallback>,
}

impl ProgressEmitter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, cb: ProgressCallback) {
        self.callback = Some(cb);
    }

    pub fn clear(&mut self) {
        self.callback = None;
    }

    pub fn emit(&self, stage: &'static str, message: impl Into<String>) {
        self.fire(stage, message.into(), None, None);
    }

    pub fn emit_counter(
        &self,
        stage: &'static str,
        message: impl Into<String>,
        current: u32,
        total: u32,
    ) {
        self.fire(stage, message.into(), Some(current), Some(total));
    }

    fn fire(&self, stage: &'static str, message: String, current: Option<u32>, total: Option<u32>) {
        if let Some(cb) = &self.callback {
            cb(ProgressEvent {
                stage,
                message,
                current,
                total,
            });
        }
    }
}

/// High-level client for the Microsoft DisplayCatalog API.
///
/// Wraps product lookups, package resolution via FE3, and search queries.
pub struct DisplayCatalogHandler {
    pub product_listing: Option<DisplayCatalogModel>,
    pub error: Option<String>,
    pub selected_endpoint: DCatEndpoint,
    pub result: Option<DisplayCatalogResult>,
    pub device_family: DeviceFamily,
    pub search_result: Option<DCatSearch>,
    pub id: Option<String>,
    pub selected_locale: Locale,
    pub is_found: bool,
    client: reqwest::Client,
    pub(crate) config: ClientConfig,
    /// Subscribe with `handler.progress.set(Box::new(|e| ...))`.
    pub progress: ProgressEmitter,
}

impl DisplayCatalogHandler {
    /// Create a new handler pointing at the given endpoint with the given
    /// locale, using the default [`ClientConfig`].
    pub fn new(endpoint: DCatEndpoint, locale: Locale) -> Self {
        Self::with_config(endpoint, locale, ClientConfig::default())
    }

    /// Create a new handler with explicit [`ClientConfig`] (timeout, retry
    /// policy, user agent).
    pub fn with_config(endpoint: DCatEndpoint, locale: Locale, config: ClientConfig) -> Self {
        let client = Self::build_client(&config);
        DisplayCatalogHandler {
            product_listing: None,
            error: None,
            selected_endpoint: endpoint,
            result: None,
            device_family: DeviceFamily::Desktop,
            search_result: None,
            id: None,
            selected_locale: locale,
            is_found: false,
            client,
            config,
            progress: ProgressEmitter::new(),
        }
    }

    /// Returns a reference to the handler's [`ClientConfig`].
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// Convenience constructor for the production endpoint with the default
    /// US/en locale.
    pub fn production() -> Self {
        Self::new(DCatEndpoint::Production, Locale::production())
    }

    // -----------------------------------------------------------------------
    // Typed accessors — convenience walks over `product_listing`
    // -----------------------------------------------------------------------
    //
    // All accessors return `Option<&T>` (or `Vec<&T>` for fan-out cases) — no
    // allocations beyond the Vec for fan-outs. They never panic; missing
    // nodes anywhere in the path yield `None`/`vec![]`.

    /// First product on the currently-loaded listing. Prefers `Products[0]`,
    /// falls back to the single `Product` field.
    pub fn product(&self) -> Option<&Product> {
        let listing = self.product_listing.as_ref()?;
        listing
            .products
            .as_deref()
            .and_then(|v| v.first())
            .or(listing.product.as_ref())
    }

    /// First localized property on the current product (typically the one
    /// matching `selected_locale.language`, in MS Store's locale fallback order).
    pub fn localized(&self) -> Option<&ProductLocalizedProperty> {
        self.product()?.localized_properties.as_deref()?.first()
    }

    /// Title from the first localized property.
    pub fn title(&self) -> Option<&str> {
        self.localized()?.product_title.as_deref()
    }

    /// Long description from the first localized property.
    pub fn description(&self) -> Option<&str> {
        self.localized()?.product_description.as_deref()
    }

    /// Publisher name from the first localized property.
    pub fn publisher_name(&self) -> Option<&str> {
        self.localized()?.publisher_name.as_deref()
    }

    /// Every image across all localized properties whose `image_purpose`
    /// matches `purpose` (case-sensitive — pass canonical PascalCase like
    /// `"Logo"`, `"Tile"`, `"Screenshot"`).
    pub fn images_with_purpose(&self, purpose: &str) -> Vec<&Image> {
        self.product()
            .and_then(|p| p.localized_properties.as_deref())
            .into_iter()
            .flatten()
            .flat_map(|lp| lp.images.as_deref().unwrap_or(&[]))
            .filter(|img| img.image_purpose.as_deref() == Some(purpose))
            .collect()
    }

    /// First SKU on the first display-sku availability.
    pub fn sku(&self) -> Option<&Sku> {
        self.product()?
            .display_sku_availabilities
            .as_deref()?
            .first()?
            .sku
            .as_ref()
    }

    /// Every [`Availability`] flattened across all display-sku availabilities.
    pub fn availabilities(&self) -> Vec<&Availability> {
        self.product()
            .and_then(|p| p.display_sku_availabilities.as_deref())
            .into_iter()
            .flatten()
            .flat_map(|dsa| dsa.availabilities.as_deref().unwrap_or(&[]))
            .collect()
    }

    /// Every [`Price`] flattened across all availabilities.
    pub fn prices(&self) -> Vec<&Price> {
        self.availabilities()
            .into_iter()
            .filter_map(|a| a.order_management_data.as_ref()?.price.as_ref())
            .collect()
    }

    /// First [`Price`] found while walking availabilities. The typical
    /// "what's the price?" shortcut.
    pub fn price(&self) -> Option<&Price> {
        self.prices().into_iter().next()
    }

    /// Packages on the first SKU's properties (empty slice if absent).
    pub fn packages(&self) -> &[Package] {
        self.sku()
            .and_then(|sku| sku.properties.as_ref())
            .and_then(|props| props.packages.as_deref())
            .unwrap_or(&[])
    }

    /// Every distinct framework / runtime dependency declared across the
    /// current product's packages — DisplayCatalog's `FrameworkDependencies`.
    ///
    /// This is the *named* dependency map: each entry carries the dependency's
    /// package-identity (PFN base, e.g. `"Microsoft.VCLibs.140.00"`,
    /// `"Microsoft.WindowsAppRuntime.1.6"`) plus the minimum version the app
    /// was built against (a packed quad-version `i64`/string). Deduplicated by
    /// `package_identity` — the same framework is repeated on every
    /// architecture package, so this collapses them to one entry each.
    ///
    /// For FE3's raw category-id dependency edges (the same graph expressed as
    /// Windows-Update category GUIDs) see
    /// [`crate::models::fe3::PackageInstance::prerequisites`].
    pub fn framework_dependencies(&self) -> Vec<&FrameworkDependency> {
        let mut seen = std::collections::HashSet::new();
        self.packages()
            .iter()
            .flat_map(|p| p.framework_dependencies.as_deref().unwrap_or(&[]))
            .filter(|d| match d.package_identity.as_deref() {
                Some(id) => seen.insert(id.to_owned()),
                None => true,
            })
            .collect()
    }

    /// Every distinct platform dependency (e.g. `Windows.Universal`,
    /// `Windows.Desktop`) declared across the current product's packages —
    /// DisplayCatalog's `PlatformDependencies`. Deduplicated by
    /// `platform_name`.
    pub fn platform_dependencies(&self) -> Vec<&PlatformDependency> {
        let mut seen = std::collections::HashSet::new();
        self.packages()
            .iter()
            .flat_map(|p| p.platform_dependencies.as_deref().unwrap_or(&[]))
            .filter(|d| match d.platform_name.as_deref() {
                Some(n) => seen.insert(n.to_owned()),
                None => true,
            })
            .collect()
    }

    /// `WuCategoryId` from the first SKU's fulfillment data. This is what
    /// `get_packages_for_product` uses internally to query FE3.
    pub fn wu_category_id(&self) -> Option<&str> {
        self.sku()?
            .properties
            .as_ref()?
            .fulfillment_data
            .as_ref()?
            .wu_category_id
            .as_deref()
    }

    /// Convenience: last-modified date on the current product, if any.
    pub fn last_modified_date(&self) -> Option<&str> {
        self.product()?.last_modified_date.as_deref()
    }

    fn build_client(config: &ClientConfig) -> reqwest::Client {
        #[cfg(not(target_arch = "wasm32"))]
        let builder = {
            let mut b = reqwest::Client::builder()
                .user_agent(&config.user_agent)
                .timeout(config.timeout);
            // fe3.delivery.mp.microsoft.com's cert chains to "Microsoft Root
            // Certificate Authority 2011", a Microsoft-internal root that
            // ships with Windows Update but is absent from the Mozilla
            // webpki-roots and the default Ubuntu CA bundle. Without it, FE3
            // SOAP calls fail with UnknownIssuer / "unable to get local
            // issuer certificate" on Linux CI. Bundle the root so trust is
            // portable across every OS. displaycatalog.mp.microsoft.com
            // chains to DigiCert Global Root G2 and is unaffected.
            const MS_ROOT_CA_2011_PEM: &[u8] = include_bytes!("../../data/ms_root_ca_2011.pem");
            if let Ok(cert) = reqwest::Certificate::from_pem(MS_ROOT_CA_2011_PEM) {
                b = b.add_root_certificate(cert);
            }
            b
        };
        #[cfg(target_arch = "wasm32")]
        let builder = reqwest::Client::builder().user_agent(&config.user_agent);
        builder.build().unwrap_or_default()
    }

    // -----------------------------------------------------------------------
    // Product query
    // -----------------------------------------------------------------------

    /// Query DisplayCatalog for a product by its `id` and `id_type`.
    ///
    /// On success `self.product_listing` and `self.is_found` are updated.
    /// An optional `auth_token` may be provided for flighted/sandbox queries.
    pub async fn query_dcat(
        &mut self,
        id: &str,
        id_type: IdentifierType,
        auth_token: Option<&str>,
    ) -> Result<(), StoreError> {
        self.query_dcat_with_cancel(id, id_type, auth_token, None)
            .await
    }

    /// Same as [`Self::query_dcat`] but races the request against an optional
    /// [`CancellationToken`]. When the token is cancelled before the request
    /// completes, dropping the in-flight future cancels the underlying HTTP
    /// request and this method returns [`StoreError::Cancelled`].
    pub async fn query_dcat_with_cancel(
        &mut self,
        id: &str,
        id_type: IdentifierType,
        auth_token: Option<&str>,
        cancel: Option<&CancellationToken>,
    ) -> Result<(), StoreError> {
        race_cancel(self.query_dcat_inner(id, id_type, auth_token), cancel).await
    }

    async fn query_dcat_inner(
        &mut self,
        id: &str,
        id_type: IdentifierType,
        auth_token: Option<&str>,
    ) -> Result<(), StoreError> {
        self.id = Some(id.to_owned());
        self.result = None;
        self.is_found = false;

        let url = create_dcat_uri(&self.selected_endpoint, id, &id_type, &self.selected_locale);
        debug!("DCat query: GET {url}");
        self.progress.emit("dcat.request", format!("GET id={id}"));

        let auth = auth_token.filter(|t| !t.is_empty());
        let response = send_with_retry(
            || {
                let mut r = self.client.get(&url);
                if let Some(token) = auth {
                    r = r.header("Authentication", token);
                }
                r
            },
            &self.config,
            None,
            |stage, msg| self.progress.emit(stage, msg),
        )
        .await
        .map_err(|e| {
            if matches!(e, StoreError::TimedOut) {
                warn!("DCat query timed out for id={id}");
            }
            e
        })?;

        let status = response.status();
        debug!("DCat response: HTTP {status}");
        self.progress
            .emit("dcat.response", format!("HTTP {status}"));

        if status.is_success() {
            let body = response.text().await.map_err(StoreError::Http)?;
            debug!("DCat response body: {} bytes", body.len());
            self.progress
                .emit("dcat.parse", format!("{} bytes", body.len()));
            let model: DisplayCatalogModel = serde_json::from_str(&body).map_err(|e| {
                error!("DCat JSON parse error: {e}");
                log_json_context(&body, e.column());
                StoreError::Json(e)
            })?;
            let title = model
                .products
                .as_deref()
                .and_then(|v| v.first())
                .or(model.product.as_ref())
                .and_then(|p| p.localized_properties.as_deref())
                .and_then(|v| v.first())
                .and_then(|lp| lp.product_title.as_deref())
                .unwrap_or("<no title>");
            info!("DCat found: \"{title}\" (id={id})");
            self.progress.emit("dcat.done", format!("\"{title}\""));
            self.product_listing = Some(model);
            self.result = Some(DisplayCatalogResult::Found);
            self.is_found = true;

            // Verbose dependency-map reporting: surface the declared framework
            // / platform dependency counts as soon as the listing is parsed,
            // so subscribers see the dependency picture without a second call.
            let fw = self.framework_dependencies();
            let plat = self.platform_dependencies();
            debug!(
                "DCat dependencies: {} framework, {} platform",
                fw.len(),
                plat.len(),
            );
            for d in &fw {
                debug!(
                    "  framework dep: {} (minVersion {:?})",
                    d.package_identity.as_deref().unwrap_or("<unknown>"),
                    d.min_version,
                );
            }
            self.progress.emit_counter(
                "dcat.dependencies",
                format!("{} framework, {} platform", fw.len(), plat.len()),
                fw.len() as u32,
                (fw.len() + plat.len()) as u32,
            );
            Ok(())
        } else if status == reqwest::StatusCode::NOT_FOUND {
            warn!("DCat: product not found (id={id})");
            self.progress.emit("dcat.notFound", format!("id={id}"));
            self.result = Some(DisplayCatalogResult::NotFound);
            Err(StoreError::NotFound)
        } else {
            let body = response.text().await.unwrap_or_default();
            Err(StoreError::Other(format!(
                "Failed to query DisplayCatalog endpoint {:?}, status {}, body: {}",
                self.selected_endpoint, status, body
            )))
        }
    }

    // -----------------------------------------------------------------------
    // Batch product query (bigIds)
    // -----------------------------------------------------------------------

    /// Query DisplayCatalog for many products in a single round-trip via the
    /// `bigIds` parameter. `ids` accepts Microsoft Store Product IDs only —
    /// alternate identifiers (PFN, ContentId, etc.) aren't supported by the
    /// batch endpoint.
    ///
    /// Populates `self.product_listing.products` with the response.
    /// Call [`Self::products`] afterwards to read the typed result.
    pub async fn query_dcat_batch(
        &mut self,
        ids: &[&str],
        auth_token: Option<&str>,
    ) -> Result<(), StoreError> {
        self.query_dcat_batch_with_cancel(ids, auth_token, None)
            .await
    }

    /// Cancellable variant of [`Self::query_dcat_batch`].
    pub async fn query_dcat_batch_with_cancel(
        &mut self,
        ids: &[&str],
        auth_token: Option<&str>,
        cancel: Option<&CancellationToken>,
    ) -> Result<(), StoreError> {
        race_cancel(self.query_dcat_batch_inner(ids, auth_token), cancel).await
    }

    async fn query_dcat_batch_inner(
        &mut self,
        ids: &[&str],
        auth_token: Option<&str>,
    ) -> Result<(), StoreError> {
        if ids.is_empty() {
            return Err(StoreError::Other(
                "query_dcat_batch: ids must be non-empty".into(),
            ));
        }

        self.id = None;
        self.result = None;
        self.is_found = false;

        let url = crate::utilities::helpers::create_dcat_batch_uri(
            &self.selected_endpoint,
            ids,
            &self.selected_locale,
        );
        debug!("DCat batch query: GET {url}");
        self.progress
            .emit("dcat.request", format!("batch GET ({} ids)", ids.len()));

        let auth = auth_token.filter(|t| !t.is_empty());
        let response = send_with_retry(
            || {
                let mut r = self.client.get(&url);
                if let Some(token) = auth {
                    r = r.header("Authentication", token);
                }
                r
            },
            &self.config,
            None,
            |stage, msg| self.progress.emit(stage, msg),
        )
        .await?;

        let status = response.status();
        debug!("DCat batch response: HTTP {status}");
        self.progress
            .emit("dcat.response", format!("HTTP {status}"));

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(StoreError::Other(format!(
                "Failed batch DisplayCatalog query for {} id(s), status {status}, body: {body}",
                ids.len(),
            )));
        }

        let body = response.text().await.map_err(StoreError::Http)?;
        self.progress
            .emit("dcat.parse", format!("{} bytes", body.len()));

        let model: DisplayCatalogModel = serde_json::from_str(&body).map_err(|e| {
            error!("DCat batch JSON parse error: {e}");
            log_json_context(&body, e.column());
            StoreError::Json(e)
        })?;

        let count = model.products.as_deref().map(|v| v.len()).unwrap_or(0);
        info!(
            "DCat batch: {count} product(s) for {} requested id(s)",
            ids.len()
        );
        self.progress
            .emit("dcat.done", format!("{count} product(s)"));

        self.product_listing = Some(model);
        self.result = Some(DisplayCatalogResult::Found);
        self.is_found = count > 0;
        Ok(())
    }

    /// All products from the most recent query (single or batch). Empty
    /// slice if no listing is loaded or it contains no `Products` array.
    pub fn products(&self) -> &[Product] {
        self.product_listing
            .as_ref()
            .and_then(|m| m.products.as_deref())
            .unwrap_or(&[])
    }

    // -----------------------------------------------------------------------
    // Package resolution via FE3
    // -----------------------------------------------------------------------

    /// Resolve the direct download URLs for the currently-loaded product.
    ///
    /// Requires `query_dcat` to have been called successfully first.
    pub async fn get_packages_for_product(
        &self,
        msa_token: Option<&str>,
    ) -> Result<Vec<PackageInstance>, StoreError> {
        self.get_packages_for_product_with_cancel(msa_token, None)
            .await
    }

    /// Same as [`Self::get_packages_for_product`] but races the FE3 SOAP call
    /// sequence against an optional [`CancellationToken`].
    pub async fn get_packages_for_product_with_cancel(
        &self,
        msa_token: Option<&str>,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<PackageInstance>, StoreError> {
        race_cancel(self.get_packages_for_product_inner(msa_token), cancel).await
    }

    async fn get_packages_for_product_inner(
        &self,
        msa_token: Option<&str>,
    ) -> Result<Vec<PackageInstance>, StoreError> {
        let product = self.product().ok_or_else(|| {
            StoreError::Other("Cannot get packages: product data is null.".into())
        })?;
        let wu_category_id = self.wu_category_id().ok_or_else(|| {
            StoreError::Other(
                "Cannot get packages: FulfillmentData (WuCategoryId) is missing.".into(),
            )
        })?;

        debug!("FE3: WuCategoryId={wu_category_id}");
        self.progress
            .emit("fe3.start", format!("WuCategoryId={wu_category_id}"));

        self.progress.emit("fe3.getCookie", "POST GetCookie");
        let cookie = FE3Handler::get_cookie(&self.client).await?;

        self.progress
            .emit("fe3.syncUpdates", format!("cookie {} bytes", cookie.len()));
        let xml =
            FE3Handler::sync_updates_with_cookie(&cookie, wu_category_id, msa_token, &self.client)
                .await?;

        self.progress
            .emit("fe3.parseUpdateIds", format!("{} bytes XML", xml.len()));
        let (update_ids, revision_ids) = FE3Handler::process_update_ids(&xml)?;
        debug!("FE3: {} update ID(s) parsed", update_ids.len());
        self.progress.emit_counter(
            "fe3.parseUpdateIds.done",
            "update IDs parsed",
            update_ids.len() as u32,
            update_ids.len() as u32,
        );

        self.progress
            .emit("fe3.parsePackages", "parsing package instances");
        let mut instances = FE3Handler::get_package_instances(&xml).await?;
        debug!("FE3: {} package instance(s) found", instances.len());
        self.progress.emit_counter(
            "fe3.parsePackages.done",
            "package instances parsed",
            instances.len() as u32,
            instances.len() as u32,
        );

        // Verbose dependency-graph reporting: total the FE3 prerequisite edges
        // (WU category IDs) parsed onto the package instances.
        let total_prereq_edges: usize = instances.iter().map(|i| i.prerequisites.len()).sum();
        let pkgs_with_prereqs = instances
            .iter()
            .filter(|i| !i.prerequisites.is_empty())
            .count();
        debug!(
            "FE3: {total_prereq_edges} prerequisite edge(s) across {pkgs_with_prereqs}/{} package(s)",
            instances.len(),
        );
        self.progress.emit_counter(
            "fe3.prerequisites",
            format!(
                "{total_prereq_edges} dependency edge(s) across {pkgs_with_prereqs} package(s)"
            ),
            pkgs_with_prereqs as u32,
            instances.len() as u32,
        );
        // Per-package fan-out so subscribers can render a live list. Message
        // format `"<moniker> | updateId=<id>"` so the frontend can correlate
        // a later `fe3.linkReceived` (which also names the update_id) back
        // to the package row it belongs to.
        let total_pkgs = instances.len() as u32;
        for (i, inst) in instances.iter().enumerate() {
            let uid = update_ids.get(i).map(String::as_str).unwrap_or("");
            self.progress.emit_counter(
                "fe3.packageFound",
                format!(
                    "{} | updateId={} | prereqs={}",
                    inst.package_moniker,
                    uid,
                    inst.prerequisites.len(),
                ),
                (i + 1) as u32,
                total_pkgs,
            );
        }

        self.progress.emit(
            "fe3.resolveUrls",
            format!("resolving {} URLs", update_ids.len()),
        );

        // Build update_id → moniker lookup so the per-link callback can
        // attach the owning package's moniker to each live URL event.
        let moniker_by_update_id: std::collections::HashMap<&str, &str> = update_ids
            .iter()
            .zip(instances.iter())
            .map(|(uid, inst)| (uid.as_str(), inst.package_moniker.as_str()))
            .collect();

        // Stream a `fe3.linkReceived` event the moment each <FileLocation>
        // is parsed (i.e. before the next request goes out), enriched with
        // the owning package's moniker so the UI can light up the matching
        // row. Message format:
        //   "<moniker> | uri=<url> | digest=<sha1-or-?> | updateId=<id>"
        let progress = &self.progress;
        let total_req = update_ids.len();
        let per_update_locs = FE3Handler::get_file_locations_with_progress(
            &update_ids,
            &revision_ids,
            msa_token,
            &self.client,
            |idx, total, update_id, loc| {
                let moniker = moniker_by_update_id
                    .get(update_id)
                    .copied()
                    .unwrap_or("<unknown>");
                progress.emit_counter(
                    "fe3.linkReceived",
                    format!(
                        "{moniker} | uri={} | digest={} | updateId={update_id}",
                        loc.url,
                        loc.digest.as_deref().unwrap_or("?"),
                    ),
                    (idx + 1) as u32,
                    total as u32,
                );
            },
        )
        .await?;
        let total_urls: usize = per_update_locs.iter().map(|v| v.len()).sum();
        debug!(
            "FE3: {} download URL(s) resolved across {} update(s)",
            total_urls,
            per_update_locs.len(),
        );
        self.progress.emit_counter(
            "fe3.resolveUrls.done",
            "URLs resolved",
            total_urls as u32,
            total_req as u32,
        );

        // Build a moniker → file-size lookup from the DCat catalog packages.
        // PackageFullName in DCat equals PackageMoniker in FE3.
        let dcat_size_map: std::collections::HashMap<&str, i64> = product
            .display_sku_availabilities
            .as_deref()
            .iter()
            .flat_map(|v| v.iter())
            .flat_map(|dsa| {
                dsa.sku
                    .as_ref()
                    .and_then(|s| s.properties.as_ref())
                    .and_then(|p| p.packages.as_deref())
                    .unwrap_or(&[])
            })
            .filter_map(|pkg| {
                let name = pkg.package_full_name.as_deref()?;
                let size = pkg.max_download_size_in_bytes?;
                Some((name, size))
            })
            .collect();

        debug!("DCat size map ({} entries):", dcat_size_map.len());
        for (name, size) in &dcat_size_map {
            debug!("  DCat package: {name} = {size} bytes");
        }
        debug!("FE3 package monikers ({} entries):", instances.len());
        for inst in &instances {
            debug!("  FE3 moniker: {}", inst.package_moniker);
        }

        for (i, instance) in instances.iter_mut().enumerate() {
            instance.update_id = update_ids.get(i).cloned().unwrap_or_default();

            // Attach every <FileLocation> FE3 returned for this update.
            let locs = per_update_locs.get(i).cloned().unwrap_or_default();

            // Pick the primary download URL: prefer the one whose FileDigest
            // matches the binary's <File Digest>, fall back to the first
            // location. This is more robust than the legacy 99-char URL
            // heuristic and correctly handles signed/secured alt URLs.
            instance.package_uri = match instance.digest.as_deref() {
                Some(want) => locs
                    .iter()
                    .find(|l| l.digest.as_deref() == Some(want))
                    .map(|l| l.url.clone())
                    .or_else(|| locs.first().map(|l| l.url.clone())),
                None => locs.first().map(|l| l.url.clone()),
            };
            instance.all_file_locations = locs;

            // file_size is already populated by the parser from <File Size>
            // / <ExtendedProperties MaxDownloadSize>. Only fall back to
            // DCat when both are missing (rare; mainly for old framework
            // packages without an ExtendedProperties size).
            if instance.file_size.is_none() {
                instance.file_size = dcat_size_map
                    .get(instance.package_moniker.as_str())
                    .copied();
            }

            debug!(
                "  package[{i}]: moniker={} digest={:?} size={:?} dcat_size={:?} locs={}",
                instance.package_moniker,
                instance.digest,
                instance.file_size,
                dcat_size_map
                    .get(instance.package_moniker.as_str())
                    .copied(),
                instance.all_file_locations.len(),
            );
            self.progress.emit_counter(
                "fe3.packageResolved",
                format!(
                    "{} | uri={} | size={} | digest={} | locs={} | prereqs={} | updateId={}",
                    instance.package_moniker,
                    instance.package_uri.as_deref().unwrap_or("<none>"),
                    instance
                        .file_size
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "?".into()),
                    instance.digest.as_deref().unwrap_or("?"),
                    instance.all_file_locations.len(),
                    instance.prerequisites.len(),
                    instance.update_id,
                ),
                (i + 1) as u32,
                total_pkgs,
            );
        }

        info!("Resolved {} package(s)", instances.len());
        self.progress.emit(
            "fe3.done",
            format!("{} package(s) resolved", instances.len()),
        );
        Ok(instances)
    }

    // -----------------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------------

    /// Search DisplayCatalog for the given query string.
    pub async fn search_dcat(
        &mut self,
        query: &str,
        device_family: DeviceFamily,
    ) -> Result<DCatSearch, StoreError> {
        self.search_dcat_paged(query, device_family, 0).await
    }

    /// Same as [`Self::search_dcat`] but races against an optional
    /// [`CancellationToken`].
    pub async fn search_dcat_with_cancel(
        &mut self,
        query: &str,
        device_family: DeviceFamily,
        cancel: Option<&CancellationToken>,
    ) -> Result<DCatSearch, StoreError> {
        self.search_dcat_paged_with_cancel(query, device_family, 0, cancel)
            .await
    }

    /// Search DisplayCatalog for the given query string, skipping `skip_count`
    /// results.
    ///
    /// Note: the upstream `productFamilies/autosuggest` endpoint caps results
    /// at ~10 entries and currently ignores the `skipItems` parameter. Calling
    /// this with `skip_count > 0` is accepted by the server but typically
    /// returns the same first page — autosuggest is type-ahead, not a
    /// paginated catalog browse. Kept for forward-compat in case Microsoft
    /// re-enables pagination, and for callers explicitly mirroring the
    /// original StoreLib query shape.
    pub async fn search_dcat_paged(
        &mut self,
        query: &str,
        device_family: DeviceFamily,
        skip_count: u32,
    ) -> Result<DCatSearch, StoreError> {
        self.search_dcat_paged_with_cancel(query, device_family, skip_count, None)
            .await
    }

    /// Same as [`Self::search_dcat_paged`] but races against an optional
    /// [`CancellationToken`].
    pub async fn search_dcat_paged_with_cancel(
        &mut self,
        query: &str,
        device_family: DeviceFamily,
        skip_count: u32,
        cancel: Option<&CancellationToken>,
    ) -> Result<DCatSearch, StoreError> {
        race_cancel(
            self.search_dcat_paged_inner(query, device_family, skip_count),
            cancel,
        )
        .await
    }

    async fn search_dcat_paged_inner(
        &mut self,
        query: &str,
        device_family: DeviceFamily,
        skip_count: u32,
    ) -> Result<DCatSearch, StoreError> {
        let base = endpoint_to_search_url(&self.selected_endpoint);
        let dep = device_family.platform_dependency_name();

        let mut url = format!(
            "{}{}&productFamilyNames=apps,games&platformDependencyName={}",
            base, query, dep
        );
        if skip_count > 0 {
            url.push_str(&format!("&skipItems={}", skip_count));
        }

        debug!("DCat search: GET {url}");
        self.progress.emit("search.request", format!("\"{query}\""));

        let response = send_with_retry(
            || self.client.get(&url),
            &self.config,
            None,
            |stage, msg| self.progress.emit(stage, msg),
        )
        .await?;

        let status = response.status();
        debug!("DCat search response: HTTP {status}");
        self.progress
            .emit("search.response", format!("HTTP {status}"));

        if status.is_success() {
            let body = response.text().await.map_err(StoreError::Http)?;
            debug!("DCat search response body: {} bytes", body.len());
            self.progress
                .emit("search.parse", format!("{} bytes", body.len()));
            self.result = Some(DisplayCatalogResult::Found);
            let result: DCatSearch = serde_json::from_str(&body).map_err(|e| {
                error!("DCat search JSON parse error: {e}");
                log_json_context(&body, e.column());
                StoreError::Json(e)
            })?;
            let count = result.total_result_count.unwrap_or(0);
            info!("DCat search: {count} result(s) for \"{query}\"");
            self.progress
                .emit("search.done", format!("{count} result(s)"));
            Ok(result)
        } else {
            let body = response.text().await.unwrap_or_default();
            Err(StoreError::Other(format!(
                "Failed to search DisplayCatalog for {:?}, status {}, body: {}",
                device_family, status, body
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Log a 200-byte window of JSON centred on `col` (1-based byte offset on
/// line 1, as reported by `serde_json::Error::column()`).
///
/// Emits two log lines:
///   ERROR  … the ~40 chars immediately around the offending token
///   DEBUG  … a wider 200-char window for surrounding context
fn log_json_context(body: &str, col: usize) {
    // serde_json column() is 1-based; clamp to valid range.
    let pos = col.saturating_sub(1).min(body.len());

    // Narrow window: show the token itself and a few chars on each side.
    const NARROW: usize = 40;
    let narrow_start = pos.saturating_sub(NARROW);
    let narrow_end = (pos + NARROW).min(body.len());
    let narrow = &body[narrow_start..narrow_end];
    let arrow_offset = pos - narrow_start;
    error!(
        "JSON context (col {}): …{}…\n{:>width$}",
        col,
        narrow,
        "^",
        width = arrow_offset + 1
    );

    // Wide window: enough context to see the enclosing field name.
    const WIDE: usize = 200;
    let wide_start = pos.saturating_sub(WIDE);
    let wide_end = (pos + WIDE).min(body.len());
    debug!(
        "JSON wide context (cols {}–{}):\n{}",
        wide_start,
        wide_end,
        &body[wide_start..wide_end]
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    // -- ClientConfig --------------------------------------------------------

    #[test]
    fn client_config_default_matches_docs() {
        let cfg = ClientConfig::default();
        assert_eq!(cfg.timeout, Duration::from_secs(30));
        assert_eq!(cfg.max_retries, 3);
        assert_eq!(cfg.initial_backoff, Duration::from_millis(500));
        assert_eq!(cfg.max_backoff, Duration::from_secs(5));
        assert_eq!(cfg.retry_on_status, vec![408, 429, 502, 503, 504]);
        assert_eq!(cfg.user_agent, "StoreLib");
    }

    #[test]
    fn backoff_for_doubles_until_cap() {
        let cfg = ClientConfig {
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(800),
            ..Default::default()
        };
        assert_eq!(cfg.backoff_for(0), Duration::from_millis(100));
        assert_eq!(cfg.backoff_for(1), Duration::from_millis(200));
        assert_eq!(cfg.backoff_for(2), Duration::from_millis(400));
        // 800ms is the cap; attempt 3 would be 800ms (cap), attempt 4+ stays at cap.
        assert_eq!(cfg.backoff_for(3), Duration::from_millis(800));
        assert_eq!(cfg.backoff_for(4), Duration::from_millis(800));
        assert_eq!(cfg.backoff_for(50), Duration::from_millis(800));
    }

    #[test]
    fn backoff_for_handles_huge_attempt_without_overflow() {
        let cfg = ClientConfig::default();
        // Should not panic on overflow.
        let _ = cfg.backoff_for(64);
        let _ = cfg.backoff_for(u32::MAX);
    }

    #[test]
    fn with_config_keeps_overrides() {
        let cfg = ClientConfig {
            user_agent: "TestUA/1.0".into(),
            max_retries: 7,
            ..Default::default()
        };
        let h = DisplayCatalogHandler::with_config(
            DCatEndpoint::Production,
            Locale::production(),
            cfg.clone(),
        );
        assert_eq!(h.config().max_retries, 7);
        assert_eq!(h.config().user_agent, "TestUA/1.0");
    }

    // -- race_cancel ---------------------------------------------------------

    #[tokio::test]
    async fn race_cancel_returns_op_result_when_not_cancelled() {
        let op = async { Ok::<_, StoreError>(42_u32) };
        let result = race_cancel(op, None).await.unwrap();
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn race_cancel_returns_op_result_when_token_uncancelled() {
        let token = CancellationToken::new();
        let op = async { Ok::<_, StoreError>("done") };
        let result = race_cancel(op, Some(&token)).await.unwrap();
        assert_eq!(result, "done");
    }

    #[tokio::test]
    async fn race_cancel_returns_cancelled_when_token_fires_first() {
        let token = CancellationToken::new();
        let canceller = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            canceller.cancel();
        });
        // Op never completes on its own.
        let op = async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok::<_, StoreError>(())
        };
        let err = race_cancel(op, Some(&token)).await.unwrap_err();
        assert!(matches!(err, StoreError::Cancelled));
    }

    #[tokio::test]
    async fn race_cancel_returns_cancelled_when_token_already_cancelled() {
        let token = CancellationToken::new();
        token.cancel();
        // Op would succeed if allowed to run, but token is already cancelled
        // so race_cancel resolves immediately on the Right branch.
        let op = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok::<_, StoreError>(())
        };
        let err = race_cancel(op, Some(&token)).await.unwrap_err();
        assert!(matches!(err, StoreError::Cancelled));
    }

    #[tokio::test]
    async fn race_cancel_propagates_op_error() {
        let token = CancellationToken::new();
        let op = async { Err::<u32, _>(StoreError::NotFound) };
        let err = race_cancel(op, Some(&token)).await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound));
    }

    // -- typed accessors -----------------------------------------------------

    /// Build a handler with a minimal known product listing for accessor tests.
    fn handler_with_listing(json: &str) -> DisplayCatalogHandler {
        let model: DisplayCatalogModel = serde_json::from_str(json).unwrap();
        let mut h = DisplayCatalogHandler::production();
        h.product_listing = Some(model);
        h.is_found = true;
        h
    }

    #[test]
    fn accessors_return_none_on_empty_handler() {
        let h = DisplayCatalogHandler::production();
        assert!(h.product().is_none());
        assert!(h.title().is_none());
        assert!(h.price().is_none());
        assert!(h.wu_category_id().is_none());
        assert!(h.packages().is_empty());
        assert!(h.availabilities().is_empty());
        assert!(h.products().is_empty());
    }

    #[test]
    fn title_publisher_description_walk_localized_properties() {
        let json = r#"{
            "Products":[{
                "LocalizedProperties":[{
                    "ProductTitle":"Netflix",
                    "ProductDescription":"Watch shows",
                    "PublisherName":"Netflix, Inc."
                }]
            }]
        }"#;
        let h = handler_with_listing(json);
        assert_eq!(h.title(), Some("Netflix"));
        assert_eq!(h.publisher_name(), Some("Netflix, Inc."));
        assert_eq!(h.description(), Some("Watch shows"));
    }

    #[test]
    fn product_falls_back_to_single_product_field() {
        // No `Products` array — only the singular `Product` field.
        let json = r#"{
            "Product":{
                "LocalizedProperties":[{"ProductTitle":"FromProduct"}]
            }
        }"#;
        let h = handler_with_listing(json);
        assert!(h.product().is_some());
        assert_eq!(h.title(), Some("FromProduct"));
    }

    #[test]
    fn price_walks_to_first_availability() {
        let json = r#"{
            "Products":[{
                "DisplaySkuAvailabilities":[{
                    "Sku":{"Properties":{}},
                    "Availabilities":[{
                        "OrderManagementData":{
                            "Price":{"CurrencyCode":"USD","MSRP":9.99,"ListPrice":4.99}
                        }
                    }]
                }]
            }]
        }"#;
        let h = handler_with_listing(json);
        let p = h.price().expect("price should be present");
        assert_eq!(p.currency_code.as_deref(), Some("USD"));
        assert_eq!(p.msrp, Some(9.99));
        assert_eq!(p.list_price, Some(4.99));
        // The fan-out version returns the same single price.
        assert_eq!(h.prices().len(), 1);
    }

    #[test]
    fn packages_and_wu_category_id_walk_sku_properties() {
        let json = r#"{
            "Products":[{
                "DisplaySkuAvailabilities":[{
                    "Sku":{"Properties":{
                        "FulfillmentData":{"WuCategoryId":"cat-abc"},
                        "Packages":[
                            {"PackageFullName":"X.Y_1","MaxDownloadSizeInBytes":1234},
                            {"PackageFullName":"X.Y_2","MaxDownloadSizeInBytes":5678}
                        ]
                    }}
                }]
            }]
        }"#;
        let h = handler_with_listing(json);
        assert_eq!(h.wu_category_id(), Some("cat-abc"));
        assert_eq!(h.packages().len(), 2);
        assert_eq!(h.packages()[0].package_full_name.as_deref(), Some("X.Y_1"),);
        assert_eq!(h.packages()[1].max_download_size_in_bytes, Some(5678));
    }

    #[test]
    fn framework_and_platform_dependencies_dedup_across_packages() {
        // Two architecture packages declaring the same framework set — the
        // accessor must collapse them to one entry per package_identity /
        // platform_name (mirrors the real DCat shape for WhatsApp etc.).
        let json = r#"{
            "Products":[{
                "DisplaySkuAvailabilities":[{
                    "Sku":{"Properties":{"Packages":[
                        {
                            "PackageFullName":"App_1.0_x64__h",
                            "FrameworkDependencies":[
                                {"PackageIdentity":"Microsoft.VCLibs.140.00","MinVersion":3940651870650368,"MaxTested":0},
                                {"PackageIdentity":"Microsoft.WindowsAppRuntime.1.6","MinVersion":1688852089373523968,"MaxTested":0}
                            ],
                            "PlatformDependencies":[
                                {"PlatformName":"Windows.Universal","MinVersion":2814751014977536,"MaxTested":2814751014977536},
                                {"PlatformName":"Windows.Desktop","MinVersion":2814751014977536,"MaxTested":2814751014977536}
                            ]
                        },
                        {
                            "PackageFullName":"App_1.0_arm64__h",
                            "FrameworkDependencies":[
                                {"PackageIdentity":"Microsoft.VCLibs.140.00","MinVersion":3940651870650368,"MaxTested":0}
                            ],
                            "PlatformDependencies":[
                                {"PlatformName":"Windows.Universal","MinVersion":2814751014977536,"MaxTested":2814751014977536}
                            ]
                        }
                    ]}}
                }]
            }]
        }"#;
        let h = handler_with_listing(json);
        let fw = h.framework_dependencies();
        // VCLibs (in both packages) collapses to one; WindowsAppRuntime once.
        assert_eq!(fw.len(), 2);
        let names: Vec<&str> = fw
            .iter()
            .filter_map(|d| d.package_identity.as_deref())
            .collect();
        assert!(names.contains(&"Microsoft.VCLibs.140.00"));
        assert!(names.contains(&"Microsoft.WindowsAppRuntime.1.6"));

        let plat = h.platform_dependencies();
        assert_eq!(plat.len(), 2);
        let pnames: Vec<&str> = plat
            .iter()
            .filter_map(|d| d.platform_name.as_deref())
            .collect();
        assert!(pnames.contains(&"Windows.Universal"));
        assert!(pnames.contains(&"Windows.Desktop"));
    }

    #[test]
    fn dependency_accessors_empty_on_bare_handler() {
        let h = DisplayCatalogHandler::production();
        assert!(h.framework_dependencies().is_empty());
        assert!(h.platform_dependencies().is_empty());
    }

    #[test]
    fn images_with_purpose_filters_correctly() {
        let json = r#"{
            "Products":[{
                "LocalizedProperties":[{
                    "Images":[
                        {"ImagePurpose":"Logo","Uri":"//img/logo.png","Height":100,"Width":100},
                        {"ImagePurpose":"Tile","Uri":"//img/tile.png","Height":300,"Width":300},
                        {"ImagePurpose":"Screenshot","Uri":"//img/ss1.png","Height":720,"Width":1280},
                        {"ImagePurpose":"Screenshot","Uri":"//img/ss2.png","Height":720,"Width":1280}
                    ]
                }]
            }]
        }"#;
        let h = handler_with_listing(json);
        assert_eq!(h.images_with_purpose("Logo").len(), 1);
        assert_eq!(h.images_with_purpose("Tile").len(), 1);
        assert_eq!(h.images_with_purpose("Screenshot").len(), 2);
        assert_eq!(h.images_with_purpose("Banner").len(), 0);
    }

    #[test]
    fn products_returns_all_products_in_batch_response() {
        // Three products as a batch query would return.
        let json = r#"{
            "Products":[
                {"LocalizedProperties":[{"ProductTitle":"A"}]},
                {"LocalizedProperties":[{"ProductTitle":"B"}]},
                {"LocalizedProperties":[{"ProductTitle":"C"}]}
            ]
        }"#;
        let h = handler_with_listing(json);
        let titles: Vec<_> = h
            .products()
            .iter()
            .filter_map(|p| {
                p.localized_properties
                    .as_deref()?
                    .first()?
                    .product_title
                    .as_deref()
            })
            .collect();
        assert_eq!(titles, vec!["A", "B", "C"]);
        // `title()` still returns the first.
        assert_eq!(h.title(), Some("A"));
    }

    // -- batch query (wiremock-backed) ---------------------------------------

    #[tokio::test]
    async fn query_dcat_batch_rejects_empty_ids() {
        let mut h = DisplayCatalogHandler::production();
        let err = h.query_dcat_batch(&[], None).await.unwrap_err();
        assert!(matches!(err, StoreError::Other(_)));
    }

    #[tokio::test]
    async fn query_dcat_batch_populates_products_on_success() {
        let server = MockServer::start().await;

        let body = r#"{"Products":[
            {"LocalizedProperties":[{"ProductTitle":"A"}]},
            {"LocalizedProperties":[{"ProductTitle":"B"}]}
        ],"TotalResultCount":2}"#;

        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "application/json")
                    .set_body_string(body),
            )
            .expect(1)
            .mount(&server)
            .await;

        // Point the handler at the mock server. We achieve this by building a
        // handler that uses the mock URL directly via the batch URI builder
        // — but query_dcat_batch_inner uses `selected_endpoint` to build URLs
        // via create_dcat_batch_uri. To avoid reaching into private state,
        // exercise the inner helper directly.
        let cfg = fast_retry_cfg();
        let client = reqwest::Client::new();
        let url = format!(
            "{}?bigIds=A,B&market=US&languages=en&catalogsource=apps&fieldsTemplate=Details",
            server.uri()
        );

        let resp = send_with_retry(|| client.get(&url), &cfg, None, |_, _| {})
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let text = resp.text().await.unwrap();
        let model: DisplayCatalogModel = serde_json::from_str(&text).unwrap();
        assert_eq!(model.products.as_deref().unwrap().len(), 2);
    }

    // -- send_with_retry (wiremock-backed) ----------------------------------

    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fast_retry_cfg() -> ClientConfig {
        // Tiny backoffs so tests don't drag.
        ClientConfig {
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(2),
            max_retries: 3,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn send_with_retry_succeeds_on_first_attempt() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1) // exactly one request
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let cfg = fast_retry_cfg();
        let resp = send_with_retry(|| client.get(server.uri()), &cfg, None, |_, _| {})
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn send_with_retry_retries_on_503_then_succeeds() {
        let server = MockServer::start().await;
        // First two requests return 503; the third returns 200.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let cfg = fast_retry_cfg();
        let resp = send_with_retry(|| client.get(server.uri()), &cfg, None, |_, _| {})
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn send_with_retry_gives_up_after_max_retries() {
        let server = MockServer::start().await;
        // Every request returns 503; expect max_retries + 1 attempts total.
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .expect(4) // 1 initial + 3 retries
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let cfg = fast_retry_cfg();
        let resp = send_with_retry(|| client.get(server.uri()), &cfg, None, |_, _| {})
            .await
            .unwrap();
        // After exhausting retries we return the last response unchanged.
        assert_eq!(resp.status(), 503);
    }

    #[tokio::test]
    async fn send_with_retry_does_not_retry_on_4xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1) // 404 is not in retry_on_status by default
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let cfg = fast_retry_cfg();
        let resp = send_with_retry(|| client.get(server.uri()), &cfg, None, |_, _| {})
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn send_with_retry_emits_progress_per_attempt() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let log_cb = log.clone();
        let client = reqwest::Client::new();
        let cfg = fast_retry_cfg();
        let _ = send_with_retry(
            || client.get(server.uri()),
            &cfg,
            None,
            |stage, _msg| log_cb.lock().unwrap().push(stage.to_string()),
        )
        .await
        .unwrap();

        let stages = log.lock().unwrap().clone();
        // Two retries → two (retry.wait, retry.attempt) pairs.
        assert_eq!(
            stages,
            vec![
                "retry.wait".to_string(),
                "retry.attempt".to_string(),
                "retry.wait".to_string(),
                "retry.attempt".to_string(),
            ],
        );
    }

    #[tokio::test]
    async fn send_with_retry_cancel_during_backoff_returns_cancelled_fast() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            // We expect at most 1 hit: first 503, then we cancel during backoff
            // so the retry never fires.
            .expect(1)
            .mount(&server)
            .await;

        let cfg = ClientConfig {
            initial_backoff: Duration::from_secs(60),
            max_backoff: Duration::from_secs(60),
            max_retries: 3,
            ..Default::default()
        };

        let token = CancellationToken::new();
        let canceller = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            canceller.cancel();
        });

        let client = reqwest::Client::new();
        let started = std::time::Instant::now();
        let err = send_with_retry(|| client.get(server.uri()), &cfg, Some(&token), |_, _| {})
            .await
            .unwrap_err();
        let elapsed = started.elapsed();

        assert!(matches!(err, StoreError::Cancelled));
        // Should resolve well under the 60s backoff once the token fires.
        assert!(
            elapsed < Duration::from_secs(2),
            "cancel-during-backoff took too long: {:?}",
            elapsed,
        );
    }

    #[test]
    fn progress_event_serializes_camel_case() {
        let e = ProgressEvent {
            stage: "fe3.syncUpdates",
            message: "cookie 256 bytes".into(),
            current: None,
            total: None,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(
            json.contains("\"stage\":\"fe3.syncUpdates\""),
            "got: {json}"
        );
        assert!(
            json.contains("\"message\":\"cookie 256 bytes\""),
            "got: {json}",
        );
        assert!(json.contains("\"current\":null"), "got: {json}");
        assert!(json.contains("\"total\":null"), "got: {json}");
    }

    #[test]
    fn progress_event_counter_serializes() {
        let e = ProgressEvent {
            stage: "fe3.resolveUrls.done",
            message: "URLs resolved".into(),
            current: Some(7),
            total: Some(7),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"current\":7"), "got: {json}");
        assert!(json.contains("\"total\":7"), "got: {json}");
    }

    fn capturing_handler() -> (DisplayCatalogHandler, Arc<Mutex<Vec<ProgressEvent>>>) {
        let log = Arc::new(Mutex::new(Vec::<ProgressEvent>::new()));
        let log_cb = log.clone();
        let mut h = DisplayCatalogHandler::production();
        h.progress
            .set(Box::new(move |e| log_cb.lock().unwrap().push(e)));
        (h, log)
    }

    #[test]
    fn emit_invokes_callback() {
        let (h, log) = capturing_handler();
        h.progress.emit("dcat.request", "GET id=foo");
        let events = log.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].stage, "dcat.request");
        assert_eq!(events[0].message, "GET id=foo");
        assert_eq!(events[0].current, None);
        assert_eq!(events[0].total, None);
    }

    #[test]
    fn emit_counter_carries_progress_numbers() {
        let (h, log) = capturing_handler();
        h.progress
            .emit_counter("fe3.resolveUrls.done", "URLs resolved", 5, 12);
        let events = log.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].stage, "fe3.resolveUrls.done");
        assert_eq!(events[0].current, Some(5));
        assert_eq!(events[0].total, Some(12));
    }

    #[test]
    fn multiple_emits_preserve_order() {
        let (h, log) = capturing_handler();
        h.progress.emit("dcat.request", "step 1");
        h.progress.emit("dcat.response", "step 2");
        h.progress.emit_counter("dcat.parse", "step 3", 1, 1);
        h.progress.emit("dcat.done", "step 4");
        let stages: Vec<&str> = log.lock().unwrap().iter().map(|e| e.stage).collect();
        assert_eq!(
            stages,
            vec!["dcat.request", "dcat.response", "dcat.parse", "dcat.done"],
        );
    }

    #[test]
    fn no_callback_means_no_panic() {
        // Without a callback installed, emit must be a no-op.
        let h = DisplayCatalogHandler::production();
        h.progress.emit("x", "y");
        h.progress.emit_counter("x", "y", 1, 1);
    }

    #[test]
    fn clear_callback_stops_delivery() {
        let (mut h, log) = capturing_handler();
        h.progress.emit("first", "");
        h.progress.clear();
        h.progress.emit("second", "");
        let events = log.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].stage, "first");
    }

    #[test]
    fn set_callback_replaces_previous() {
        let log_a: Arc<Mutex<Vec<ProgressEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let log_b: Arc<Mutex<Vec<ProgressEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let mut h = DisplayCatalogHandler::production();

        let a = log_a.clone();
        h.progress.set(Box::new(move |e| a.lock().unwrap().push(e)));
        h.progress.emit("first", "");

        let b = log_b.clone();
        h.progress.set(Box::new(move |e| b.lock().unwrap().push(e)));
        h.progress.emit("second", "");

        assert_eq!(log_a.lock().unwrap().len(), 1);
        assert_eq!(log_a.lock().unwrap()[0].stage, "first");
        assert_eq!(log_b.lock().unwrap().len(), 1);
        assert_eq!(log_b.lock().unwrap()[0].stage, "second");
    }
}
