//! Acquire verified amd64/neutral app and prerequisite candidates for a test image.
//! Manifest excludes temporary locations; package selection and licenses remain caller duties.
#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use storelib_rs::{DisplayCatalogHandler, IdentifierType, SfsClient};
    let output = std::env::args()
        .nth(1)
        .ok_or("usage: sfs_acquire OUTPUT_DIRECTORY [PRODUCT_ID]")?;
    let product = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "9N0DX20HK701".into());
    std::fs::create_dir(&output)?;
    let mut catalog = DisplayCatalogHandler::production();
    catalog
        .query_dcat(&product, IdentifierType::ProductId, None)
        .await?;
    let info = catalog.get_sfs_download_info().await?;
    let client = SfsClient::new()?;
    let mut groups = vec![("app", &info.version.content_id, &info.files)];
    groups.extend(
        info.prerequisites
            .iter()
            .map(|p| ("dependency", &p.content_id, &p.files)),
    );
    let mut entries = Vec::new();
    for (role, identity, files) in groups {
        for file in files {
            if !file.metadata.applicability.architectures.iter().any(|a| {
                matches!(
                    a.to_ascii_lowercase().as_str(),
                    "amd64" | "x64" | "none" | "neutral"
                )
            }) {
                continue;
            }
            let name = format!("{:03}-{role}.appx", entries.len());
            let path = std::path::Path::new(&output).join(name);
            let bytes = client
                .download_verified(identity, &file.metadata.file_id, 256 * 1024 * 1024, None)
                .await?;
            // Download resolution reads fresh metadata; keep the saved manifest
            // tied to the original selection rather than trusting mutable service data.
            if bytes.len() as u64 != file.metadata.size_in_bytes
                || file.metadata.sha256_hex().as_deref()
                    != Some(format!("{:x}", Sha256::digest(&bytes)).as_str())
            {
                return Err("download no longer matches selected file metadata".into());
            }
            let mut package = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            package.write_all(&bytes)?;
            package.sync_all()?;
            entries.push(serde_json::json!({"path":path,"size_bytes":file.metadata.size_in_bytes,"sha256":file.metadata.sha256_hex(),"role":role,"metadata":file.metadata,"content_id":identity}));
            println!(
                "verified {} bytes={} role={role}",
                path.display(),
                file.metadata.size_in_bytes
            );
        }
    }
    let mut manifest = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(std::path::Path::new(&output).join("manifest.json"))?;
    manifest.write_all(&serde_json::to_vec_pretty(
        &serde_json::json!({"product_id":product,"packages":entries}),
    )?)?;
    manifest.sync_all()?;
    Ok(())
}
#[cfg(target_arch = "wasm32")]
fn main() {}
