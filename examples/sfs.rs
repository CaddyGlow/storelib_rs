//! Resolve public Store app metadata without printing temporary download URLs.

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use storelib_rs::{DisplayCatalogHandler, IdentifierType};
    let product_id = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "9N0DX20HK701".into());
    let mut catalog = DisplayCatalogHandler::production();
    catalog
        .query_dcat(&product_id, IdentifierType::ProductId, None)
        .await?;
    let info = catalog.get_sfs_download_info().await?;
    println!(
        "product={product_id} version={} app_files={} prerequisites={}",
        info.version.content_id.version,
        info.files.len(),
        info.prerequisites.len()
    );
    for file in &info.files {
        println!(
            "file={} bytes={} sha256={}",
            file.metadata.file_moniker,
            file.metadata.size_in_bytes,
            file.metadata
                .sha256_hex()
                .as_deref()
                .unwrap_or("unavailable")
        );
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn main() {}
