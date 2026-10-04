//! `storelib_rs` – Rust port of StoreLib, a Microsoft Store API client.
//!
//! Supports both native (tokio) and WASM (wasm-bindgen-futures) async
//! runtimes.  Enable the `wasm` feature when targeting `wasm32-unknown-unknown`.

// ---------------------------------------------------------------------------
// Public modules
// ---------------------------------------------------------------------------

pub mod cancellation;
pub mod error;
pub mod models;
pub mod services;
pub mod utilities;

#[cfg(all(feature = "ffi", not(target_arch = "wasm32")))]
pub mod ffi;

/// JS bindings exposed via wasm-bindgen. Provides `Locale`,
/// `DisplayCatalogHandler`, `Fe3Handler`, and free helper functions for use
/// from npm.
#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub mod wasm;

// ---------------------------------------------------------------------------
// Convenient re-exports at the crate root
// ---------------------------------------------------------------------------

// cancellation
pub use cancellation::{CancellationToken, Cancelled};

// error
pub use error::StoreError;

// models
pub use models::addon::Addon;
pub use models::catalog::{
    AllowedPlatform, AlternateId, ApplicableLegacyPlatform, Application, Availability,
    AvailabilityProperties, ClientConditions, CmsVideo, Conditions, ContentRating,
    DisplayCatalogModel, DisplaySkuAvailability, FrameworkDependency, FulfillmentData,
    HardwareProperties, Image, LegalText, LicensingData, OrderManagementData, Package,
    PackageDownloadUri, PiFilter, PlatformDependency, Price, Product, ProductLocalizedProperty,
    ProductMarketProperty, ProductProperties, Remediation, SatisfyingEntitlementKey, SearchTitle,
    Sku, SkuDisplayGroup, SkuLocalizedProperty, SkuMarketProperty, SkuProperties, UsageDatum,
    ValidationData, WebBlendsData,
};
pub use models::enums::{
    DCatEndpoint, DeviceFamily, DisplayCatalogResult, IdentifierType, ImagePurpose, PackageType,
    ProductKind,
};
pub use models::fe3::{
    ApplicabilityBlob, AppxFamilyMetadata, CategoryInformation, ContentTargetPlatform, Deployment,
    DigestEntry, PackageInstance, RelationshipGroup, Relationships, ResolvedFileLocation,
    UpdateProperties, UpdateRef,
};
pub use models::locale::{Lang, LanguageTag, Locale, Market};
pub use models::search::{DCatSearch, SearchResult};

// services
pub use services::display_catalog::{
    ClientConfig, DisplayCatalogHandler, ProgressCallback, ProgressEmitter, ProgressEvent,
};
pub use services::fe3::FE3Handler;
#[cfg(not(target_arch = "wasm32"))]
pub use services::sfs::{
    SfsAppDownloadInfo, SfsApplicability, SfsClient, SfsConfig, SfsContentId, SfsDownloadUrl,
    SfsError, SfsFile, SfsFileMetadata, SfsPrerequisite, SfsTargetingAttributes, SfsVersion,
};

// utilities
pub use utilities::helpers::{
    create_dcat_batch_uri, create_dcat_uri, endpoint_to_base_url, endpoint_to_search_url,
    string_to_package_type,
};
