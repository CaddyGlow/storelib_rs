use crate::models::enums::PackageType;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// FE3 SyncUpdates sub-structures (full-fidelity capture — nothing dropped)
// ---------------------------------------------------------------------------

/// One `<UpdateIdentity>` reference (carries both `UpdateID` and the
/// `RevisionNumber`, neither of which is dropped).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRef {
    pub update_id: String,
    pub revision_number: Option<String>,
}

/// A relationship entry: either a bare `<UpdateIdentity>` child
/// (`is_category` = `None`, one ref) or an `<AtLeastOne IsCategory="…">`
/// wrapper around one or more refs. Both forms appear under
/// `<Prerequisites>` and `<BundledUpdates>`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipGroup {
    /// `IsCategory` from the `<AtLeastOne>` wrapper (`None` for a bare
    /// `<UpdateIdentity>` not inside an `<AtLeastOne>`).
    pub is_category: Option<bool>,
    pub updates: Vec<UpdateRef>,
}

/// The complete `<Relationships>` block from a `SyncUpdates` update — both
/// the `<Prerequisites>` (dependency) graph and the `<BundledUpdates>`
/// (bundle → child) graph, with grouping and `IsCategory`/`RevisionNumber`
/// preserved.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Relationships {
    pub prerequisites: Vec<RelationshipGroup>,
    pub bundled_updates: Vec<RelationshipGroup>,
}

/// `<AppxFamilyMetadata>` — package-family identity that sits next to
/// `<AppxMetadata>` under `<AppxPackageMetadata>`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppxFamilyMetadata {
    pub name: Option<String>,
    pub publisher: Option<String>,
    pub legacy_mobile_product_id: Option<String>,
}

/// `<CategoryInformation>` under `<HandlerSpecificData>`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryInformation {
    pub category_type: Option<String>,
    pub display_order: Option<i64>,
    pub exclude_by_default: Option<bool>,
    pub excluded_by_default: Option<bool>,
    pub prohibits_subcategories: Option<bool>,
    pub prohibits_updates: Option<bool>,
}

/// The update-level `<Properties>` element (distinct from the per-binary
/// `<ExtendedProperties>`): deployment ranking / type flags.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProperties {
    pub apply_package_rank: Option<String>,
    pub explicitly_deployable: Option<String>,
    pub is_appx_framework: Option<bool>,
    pub package_rank: Option<i64>,
    pub per_user: Option<String>,
    pub update_type: Option<String>,
}

/// The `<Deployment>` block under `<UpdateInfo>` (server-side deployment
/// metadata for the update).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    pub id: Option<String>,
    pub action: Option<String>,
    pub is_assigned: Option<String>,
    pub last_change_time: Option<String>,
    pub auto_select: Option<String>,
    pub auto_download: Option<String>,
    pub supersedence_behavior: Option<String>,
    pub priority: Option<String>,
    pub handler_specific_action: Option<String>,
    pub flight_id: Option<String>,
}

/// A named hash returned by FE3 (`AdditionalDigest`, `PiecesHashDigest`,
/// `BlockMapDigest`). `algorithm` is the wire string (e.g. `"SHA256"`).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DigestEntry {
    pub algorithm: String,
    pub value: String,
}

/// One `<FileLocation>` from a `GetExtendedUpdateInfo2` response.
///
/// FE3 typically returns several `FileLocation`s per update — the binary's
/// download URL plus the blockmap's, and on some responses both an
/// unsigned CDN edge URL and a signed `tlu.dl.delivery.mp.microsoft.com`
/// URL with auth query params. `digest` ties the location back to a
/// specific `<File>` entry (the matching `Digest` attribute) so callers
/// can distinguish "the binary" from "the blockmap" without URL heuristics.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedFileLocation {
    pub url: String,
    /// Per-URL hash from `<FileDigest>` — base64-encoded, matches the
    /// `Digest` attribute on the `<File>` this URL serves.
    pub digest: Option<String>,
}

/// A resolved package instance with download URI and update metadata.
///
/// Almost every field beyond the first eight is sourced from the FE3
/// `SyncUpdates` response — see the parser in `services::fe3` for the
/// exact XML path. Fields are `Option`/`Vec` so absence is just `None`/
/// empty; nothing here will panic at construction time.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInstance {
    pub package_moniker: String,
    /// Primary download URL matched by update identity and file digest.
    /// `None` if the identity or digest cannot be matched. When no digest is
    /// available, a URL is selected only if the update has one file location.
    /// See [`Self::all_file_locations`] for *all* URLs FE3 returned,
    /// including blockmaps and signed/secured alternatives.
    pub package_uri: Option<String>,
    pub package_type: PackageType,
    pub applicability_blob: Option<ApplicabilityBlob>,
    /// The owning update's GUID, parsed from its `UpdateIdentity` element.
    /// Empty when the metadata does not contain an owning identity.
    pub update_id: String,
    /// Download size in bytes. Sourced from `<File Size="">` (SyncUpdates)
    /// for the primary binary, falling back to `<ExtendedProperties
    /// MaxDownloadSize="">`, then to DisplayCatalog's
    /// `MaxDownloadSizeInBytes`. `None` only when none of the three carry
    /// a value.
    #[serde(rename = "packageSize")]
    pub file_size: Option<i64>,
    /// FE3's raw `<File FileName="...">` value — typically a GUID followed
    /// by the real extension (`.appx`, `.appxbundle`, `.msixbundle`,
    /// `.eappx`, `.xap`). `None` if the SyncUpdates response had no matching
    /// `<File>` element (rare).
    pub file_name: Option<String>,
    /// Human-readable filename suitable for saving the package to disk:
    /// `<package_moniker><real extension>`, where the extension is taken
    /// from [`Self::file_name`] (whitelisted to known package formats) and
    /// defaults to `.appx` when the raw name is missing or unrecognised.
    ///
    /// Example: `4DF9E0F8.Netflix_8.156.0.0_neutral_~_mcm4njqhnhss8.appxbundle`.
    ///
    /// This is *not* sanitised for any particular filesystem — callers that
    /// write to disk should sanitise per-OS (`:` `*` `?` etc. on Windows).
    pub readable_file_name: String,

    // ---------------------------------------------------------------
    // <AppxMetadata IsAppxBundle="...">
    // ---------------------------------------------------------------
    /// `true` if the primary file is a bundle (`.appxbundle` / `.msixbundle`).
    pub is_appx_bundle: Option<bool>,

    // ---------------------------------------------------------------
    // <File> attributes (primary binary — the one whose
    //   InstallerSpecificIdentifier matches the moniker)
    // ---------------------------------------------------------------
    /// Per-binary hash — base64-encoded. Algorithm in
    /// [`Self::digest_algorithm`] (`"SHA1"` for every package observed so
    /// far). Use this to verify the download after fetching.
    pub digest: Option<String>,
    pub digest_algorithm: Option<String>,
    /// Last-modified timestamp from `<File Modified="">`, ISO 8601.
    pub modified: Option<String>,
    /// Present on companion files (blockmaps/CABs); absent on the primary
    /// binary. e.g. `"DynamicMetadata"`.
    pub patching_type: Option<String>,
    /// Extra `<AdditionalDigest Algorithm="...">value</AdditionalDigest>`
    /// children of the `<File>` element.
    pub additional_digests: Vec<DigestEntry>,
    /// `<PiecesHashDigest>` — used for delta/range downloads.
    pub pieces_hash_digest: Option<DigestEntry>,
    /// `<BlockMapDigest>` — hash of the package's `.appxblockmap.xml`.
    pub block_map_digest: Option<DigestEntry>,

    /// **Ready-to-use** lowercase-hex SHA-1 of the exact bytes FE3 serves,
    /// decoded from the primary `<File Digest>` (when `DigestAlgorithm` is
    /// SHA1) or a matching `<AdditionalDigest>`. Compare directly against
    /// `Get-FileHash -Algorithm SHA1` / `sha1sum`. `None` if FE3 carried no
    /// SHA-1 for this file. (The raw base64 lives in [`Self::digest`] /
    /// [`Self::additional_digests`].)
    pub sha1: Option<String>,
    /// **Ready-to-use** lowercase-hex SHA-256 of the exact bytes FE3 serves,
    /// decoded from `<AdditionalDigest Algorithm="SHA256">` (or the primary
    /// digest when it is SHA256). Compare directly against `Get-FileHash` /
    /// `sha256sum`. This is the authoritative checksum for the downloaded
    /// file — unlike DisplayCatalog's [`crate::models::catalog::Package::hash`],
    /// which is base64 and frequently describes a different (storefront)
    /// version. `None` if FE3 carried no SHA-256.
    pub sha256: Option<String>,

    // ---------------------------------------------------------------
    // <ExtendedProperties> attributes (the rich variant — only some
    // updates carry these; lightweight packages have them all `None`)
    // ---------------------------------------------------------------
    /// Update handler URI, e.g.
    /// `"http://schemas.microsoft.com/msus/2002/12/UpdateHandlers/AppxPackage"`.
    pub handler: Option<String>,
    /// Authoritative framework flag from `<ExtendedProperties
    /// IsAppxFramework="">`. Prefer this over moniker-prefix heuristics.
    pub is_appx_framework: Option<bool>,
    pub max_download_size: Option<i64>,
    pub min_download_size: Option<i64>,
    /// Store-side content identifier for this specific package.
    pub package_content_id: Option<String>,
    /// PFN base, e.g. `"4DF9E0F8.NETFLIX"`.
    pub package_identity_name: Option<String>,
    pub creation_date: Option<String>,
    pub content_type: Option<String>,
    pub mandatory_version: Option<String>,
    pub mandatory_date: Option<String>,
    pub default_properties_language: Option<String>,
    pub from_store_service: Option<bool>,
    pub legacy_mobile_product_id: Option<String>,

    // ---------------------------------------------------------------
    // <AppxPackageInstallData>
    // ---------------------------------------------------------------
    /// `true` for the primary package in a bundle; `false` for satellites
    /// (resource/language/scale split packages).
    pub main_package: Option<bool>,

    // ---------------------------------------------------------------
    // Update identity / <UpdateInfo> envelope
    // ---------------------------------------------------------------
    /// `RevisionNumber` from the update's own `<UpdateIdentity>`.
    pub revision_number: Option<String>,
    /// Server-side numeric `<ID>` of the owning `<UpdateInfo>` (distinct from
    /// the `<UpdateIdentity>` GUID in [`Self::update_id`]).
    pub update_info_id: Option<String>,
    /// `<IsLeaf>` flag on the `<UpdateInfo>`.
    pub is_leaf: Option<bool>,
    /// `<IsShared>` flag on the `<UpdateInfo>`.
    pub is_shared: Option<bool>,

    // ---------------------------------------------------------------
    // <File> / install data (previously-dropped fields)
    // ---------------------------------------------------------------
    /// `<File InstallerSpecificIdentifier="…">` — equals the package moniker,
    /// but preserved verbatim so no attribute is lost.
    pub installer_specific_identifier: Option<String>,
    /// `<AppxPackageInstallData PackageFileName="…">` — the FE3 file name
    /// (GUID + extension) for this package's primary binary.
    pub package_file_name: Option<String>,
    /// `<HandlerSpecificData type="…">`, e.g. `"appx:AppxInstaller"`.
    pub handler_type: Option<String>,

    // ---------------------------------------------------------------
    // <Relationships> (FE3 dependency graph) — full fidelity + flat views
    // ---------------------------------------------------------------
    /// Prerequisite Windows-Update **category** IDs for this package
    /// (flattened convenience view of `relationships.prerequisites`). These
    /// are *category* GUIDs, not update GUIDs: one is the product's own
    /// `WuCategoryId`, the rest identify the framework categories (VCLibs,
    /// WindowsAppRuntime, .NET Native, …) the package depends on. For
    /// human-readable PFNs use DisplayCatalog's
    /// [`crate::models::catalog::FrameworkDependency`].
    pub prerequisites: Vec<String>,
    /// Flattened convenience view of `relationships.bundled_updates` — the
    /// child update GUIDs this update bundles (empty for leaf packages).
    pub bundled_updates: Vec<String>,
    /// The complete `<Relationships>` block, preserving grouping,
    /// `IsCategory`, and per-ref `RevisionNumber` that the flat views drop.
    pub relationships: Relationships,

    // ---------------------------------------------------------------
    // Rich metadata blocks (previously dropped entirely)
    // ---------------------------------------------------------------
    /// Update-level `<Properties>` (deployment ranking / type flags).
    pub update_properties: Option<UpdateProperties>,
    /// `<AppxFamilyMetadata>` package-family identity.
    pub family_metadata: Option<AppxFamilyMetadata>,
    /// `<CategoryInformation>` under `<HandlerSpecificData>`.
    pub category_information: Option<CategoryInformation>,
    /// `<Deployment>` block from the `<UpdateInfo>`.
    pub deployment: Option<Deployment>,
    /// Raw inner XML of `<ApplicabilityRules>` (the `IsInstalled` /
    /// `IsInstallable` evaluation tree), preserved verbatim so its nested
    /// rules are never lost. `None` when the update had no rules block.
    pub applicability_rules_xml: Option<String>,
    /// Raw XML of `<InstallationBehavior>` when present and non-empty.
    pub installation_behavior_xml: Option<String>,

    /// Catch-all for any attribute on a parsed element that is not mapped to
    /// a typed field above — keyed `"<Element>@<Attr>"`. Guarantees no
    /// attribute is ever silently dropped, including fields Microsoft may add
    /// in the future. Empty for responses fully covered by the typed fields.
    pub extra_attributes: BTreeMap<String, String>,

    // ---------------------------------------------------------------
    // <FileLocation> entries (GetExtendedUpdateInfo2)
    // ---------------------------------------------------------------
    /// Every URL FE3 returned for this update, including blockmap URLs
    /// and signed alternatives. Useful when the primary URL is rate-limited
    /// and a fallback is needed. Empty until URLs have been resolved.
    pub all_file_locations: Vec<ResolvedFileLocation>,
}

impl PackageInstance {
    /// Return the canonical extension (`.appx`, `.appxbundle`, `.msixbundle`,
    /// `.eappx`, `.eappxbundle`, `.emsix`, `.emsixbundle`, `.msix`, `.xap`)
    /// for an FE3 `FileName` value, or `None` when the input has no
    /// extension or one that isn't in the package whitelist.
    pub fn package_extension(file_name: &str) -> Option<&'static str> {
        let dot = file_name.rfind('.')?;
        match &file_name[dot..].to_ascii_lowercase()[..] {
            ".appx" => Some(".appx"),
            ".appxbundle" => Some(".appxbundle"),
            ".msix" => Some(".msix"),
            ".msixbundle" => Some(".msixbundle"),
            ".eappx" => Some(".eappx"),
            ".eappxbundle" => Some(".eappxbundle"),
            ".emsix" => Some(".emsix"),
            ".emsixbundle" => Some(".emsixbundle"),
            ".xap" => Some(".xap"),
            _ => None,
        }
    }

    /// Build the canonical `readable_file_name` from a moniker + raw FE3
    /// FileName. Falls back to `.appx` when `file_name` is missing or has
    /// an unrecognised extension.
    pub fn build_readable_file_name(moniker: &str, file_name: Option<&str>) -> String {
        let ext = file_name
            .and_then(Self::package_extension)
            .unwrap_or(".appx");
        format!("{moniker}{ext}")
    }

    /// Decode an FE3 digest value to lowercase hex. FE3 emits digests as
    /// base64 (e.g. `"SvGxBAsz2gqDUc3zvlYYqJCElQ0="`); this returns the hex
    /// form (`"4af1b104…"`). A value that is already hex (40 or 64 chars) is
    /// passed through lowercased. Returns `None` for empty / undecodable input.
    pub fn digest_to_hex(value: &str) -> Option<String> {
        let t = value.trim();
        if t.is_empty() {
            return None;
        }
        // Already hex? (SHA-1 = 40, SHA-256 = 64 hex chars.)
        if (t.len() == 40 || t.len() == 64) && t.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(t.to_ascii_lowercase());
        }
        base64_decode(t)
            .filter(|bytes| !bytes.is_empty())
            .map(|bytes| bytes_to_hex(&bytes))
    }

    /// Canonicalize a digest-algorithm label, tolerating separators / casing
    /// (`"SHA-256"`, `"Sha 256"` → `"SHA256"`). Returns `None` for anything
    /// other than SHA-1 / SHA-256.
    fn normalize_algorithm(label: &str) -> Option<&'static str> {
        let canon: String = label
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_uppercase();
        match canon.as_str() {
            "SHA1" => Some("SHA1"),
            "SHA256" => Some("SHA256"),
            _ => None,
        }
    }

    /// Pick the lowercase-hex digest for `want` (`"SHA1"` / `"SHA256"`) from a
    /// primary `(digest, algorithm)` pair plus any `<AdditionalDigest>`
    /// entries. Prefers the primary digest when its algorithm matches, else
    /// the first matching additional digest. Used to populate
    /// [`Self::sha1`] / [`Self::sha256`].
    pub(crate) fn select_digest_hex(
        digest: Option<&str>,
        digest_algorithm: Option<&str>,
        additional: &[DigestEntry],
        want: &str,
    ) -> Option<String> {
        if let (Some(d), Some(alg)) = (digest, digest_algorithm) {
            if Self::normalize_algorithm(alg) == Some(want) {
                if let Some(hex) = Self::digest_to_hex(d) {
                    return Some(hex);
                }
            }
        }
        additional
            .iter()
            .find(|e| Self::normalize_algorithm(&e.algorithm) == Some(want))
            .and_then(|e| Self::digest_to_hex(&e.value))
    }
}

/// Decode standard-alphabet base64 (ignoring `=` padding and whitespace) to
/// bytes. Returns `None` on any invalid character.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn sextet(c: u8) -> Option<u8> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits: u8 = 0;
    for &c in s.as_bytes() {
        if matches!(c, b'=' | b'\n' | b'\r' | b' ' | b'\t') {
            continue;
        }
        acc = (acc << 6) | sextet(c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Lowercase-hex encode bytes.
fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// Applicability metadata embedded in the FE3 SyncUpdates response.
///
/// Note: the original JSON uses dot-separated keys such as
/// `"blob.version"`, `"content.isMain"` etc.  Those are preserved here
/// via explicit `#[serde(rename = "...")]` attributes.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApplicabilityBlob {
    #[serde(rename = "blob.version")]
    pub blob_version: Option<i64>,

    #[serde(rename = "content.isMain")]
    pub content_is_main: Option<bool>,

    #[serde(rename = "content.packageId")]
    pub content_package_id: Option<String>,

    #[serde(rename = "content.productId")]
    pub content_product_id: Option<String>,

    #[serde(rename = "content.targetPlatforms")]
    pub content_target_platforms: Option<Vec<ContentTargetPlatform>>,

    #[serde(rename = "content.type")]
    pub content_type: Option<i32>,
}

/// Per-platform targeting information inside an `ApplicabilityBlob`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContentTargetPlatform {
    #[serde(rename = "platform.maxVersionTested")]
    pub platform_max_version_tested: Option<i64>,

    #[serde(rename = "platform.minVersion")]
    pub platform_min_version: Option<i64>,

    #[serde(rename = "platform.target")]
    pub platform_target: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::enums::PackageType;

    fn sample_instance() -> PackageInstance {
        let moniker = "Microsoft.Test_1.0_x64";
        let file_name = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.appx";
        PackageInstance {
            package_moniker: moniker.into(),
            package_uri: Some("https://download.example/pkg.appx".into()),
            package_type: PackageType::AppX,
            update_id: "11111111-2222-3333-4444-555555555555".into(),
            file_size: Some(12345),
            file_name: Some(file_name.into()),
            readable_file_name: PackageInstance::build_readable_file_name(moniker, Some(file_name)),
            ..Default::default()
        }
    }

    #[test]
    fn package_instance_serializes_camel_case() {
        let json = serde_json::to_string(&sample_instance()).unwrap();
        assert!(json.contains("\"packageMoniker\":"), "got: {json}");
        assert!(json.contains("\"packageUri\":"), "got: {json}");
        assert!(json.contains("\"packageType\":"), "got: {json}");
        assert!(json.contains("\"applicabilityBlob\":"), "got: {json}");
        assert!(json.contains("\"updateId\":"), "got: {json}");
        // file_size is renamed to packageSize on the wire.
        assert!(json.contains("\"packageSize\":12345"), "got: {json}");
        assert!(
            json.contains("\"fileName\":\"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.appx\""),
            "got: {json}",
        );
        assert!(
            json.contains("\"readableFileName\":\"Microsoft.Test_1.0_x64.appx\""),
            "got: {json}",
        );
        // New dependency field serializes under its camelCase wire name.
        assert!(json.contains("\"prerequisites\":[]"), "got: {json}");
        // No snake_case or PascalCase leaks.
        assert!(!json.contains("file_size"), "got: {json}");
        assert!(!json.contains("file_name"), "got: {json}");
        assert!(!json.contains("readable_file_name"), "got: {json}");
        assert!(!json.contains("package_uri"), "got: {json}");
        assert!(!json.contains("PackageUri"), "got: {json}");
    }

    #[test]
    fn package_extension_whitelist() {
        assert_eq!(
            PackageInstance::package_extension("abc.appx"),
            Some(".appx")
        );
        assert_eq!(
            PackageInstance::package_extension("abc.APPXBUNDLE"),
            Some(".appxbundle"),
        );
        assert_eq!(
            PackageInstance::package_extension("abc.Msix"),
            Some(".msix"),
        );
        assert_eq!(
            PackageInstance::package_extension("abc.eappxbundle"),
            Some(".eappxbundle"),
        );
        assert_eq!(PackageInstance::package_extension("abc.xap"), Some(".xap"));
        assert_eq!(PackageInstance::package_extension("abc.weird"), None);
        assert_eq!(PackageInstance::package_extension("noext"), None);
    }

    #[test]
    fn build_readable_file_name_defaults_to_appx() {
        assert_eq!(
            PackageInstance::build_readable_file_name("Foo.Bar_1.0_x64", Some("guid.appxbundle"),),
            "Foo.Bar_1.0_x64.appxbundle",
        );
        assert_eq!(
            PackageInstance::build_readable_file_name("Foo.Bar_1.0_x64", None),
            "Foo.Bar_1.0_x64.appx",
        );
        assert_eq!(
            PackageInstance::build_readable_file_name("Foo.Bar_1.0_x64", Some("guid.weird")),
            "Foo.Bar_1.0_x64.appx",
        );
    }

    #[test]
    fn digest_to_hex_decodes_base64() {
        // Real values captured from a SyncUpdates <File>.
        assert_eq!(
            PackageInstance::digest_to_hex("SvGxBAsz2gqDUc3zvlYYqJCElQ0="),
            Some("4af1b1040b33da0a8351cdf3be5618a89084950d".to_string()),
        );
        assert_eq!(
            PackageInstance::digest_to_hex("Cdmft9WJDkWAcAH7YZGK70c++XfGi0qLtrjswxq0hF8="),
            Some("09d99fb7d5890e45807001fb61918aef473ef977c68b4a8bb6b8ecc31ab4845f".to_string()),
        );
    }

    #[test]
    fn digest_to_hex_passes_through_existing_hex_lowercased() {
        let hex40 = "4AF1B1040B33DA0A8351CDF3BE5618A89084950D";
        assert_eq!(
            PackageInstance::digest_to_hex(hex40),
            Some(hex40.to_ascii_lowercase()),
        );
        assert_eq!(PackageInstance::digest_to_hex(""), None);
    }

    #[test]
    fn select_digest_hex_matches_algorithm_tolerantly() {
        let additional = vec![DigestEntry {
            algorithm: "SHA-256".into(),
            value: "Cdmft9WJDkWAcAH7YZGK70c++XfGi0qLtrjswxq0hF8=".into(),
        }];
        // SHA-1 from the primary digest, tolerating a "Sha 1" label.
        assert_eq!(
            PackageInstance::select_digest_hex(
                Some("SvGxBAsz2gqDUc3zvlYYqJCElQ0="),
                Some("Sha 1"),
                &additional,
                "SHA1",
            ),
            Some("4af1b1040b33da0a8351cdf3be5618a89084950d".to_string()),
        );
        // SHA-256 falls back to the AdditionalDigest (labelled "SHA-256").
        assert_eq!(
            PackageInstance::select_digest_hex(
                Some("SvGxBAsz2gqDUc3zvlYYqJCElQ0="),
                Some("SHA1"),
                &additional,
                "SHA256",
            ),
            Some("09d99fb7d5890e45807001fb61918aef473ef977c68b4a8bb6b8ecc31ab4845f".to_string()),
        );
        // Absent algorithm → None (never fabricated).
        assert_eq!(
            PackageInstance::select_digest_hex(
                Some("SvGxBAsz2gqDUc3zvlYYqJCElQ0="),
                Some("SHA1"),
                &[],
                "SHA256",
            ),
            None,
        );
    }

    #[test]
    fn package_instance_package_size_null_when_unknown() {
        let mut p = sample_instance();
        p.file_size = None;
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"packageSize\":null"), "got: {json}");
    }

    #[test]
    fn package_instance_package_type_emits_camel_case_enum() {
        let mut p = sample_instance();
        p.package_type = PackageType::AppX;
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"packageType\":\"appX\""), "got: {json}");

        p.package_type = PackageType::Unknown;
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"packageType\":\"unknown\""), "got: {json}");
    }

    #[test]
    fn applicability_blob_round_trip_preserves_dotted_keys() {
        // FE3 wire format uses dot-separated keys; we deliberately pass them
        // through unchanged so consumers can correlate with the SOAP payload.
        let json = r#"{"blob.version":1,"content.isMain":true,"content.packageId":"abc"}"#;
        let blob: ApplicabilityBlob = serde_json::from_str(json).unwrap();
        assert_eq!(blob.blob_version, Some(1));
        assert_eq!(blob.content_is_main, Some(true));
        assert_eq!(blob.content_package_id.as_deref(), Some("abc"));

        let out = serde_json::to_string(&blob).unwrap();
        assert!(out.contains("\"blob.version\":1"), "got: {out}");
        assert!(out.contains("\"content.isMain\":true"), "got: {out}");
    }
}
