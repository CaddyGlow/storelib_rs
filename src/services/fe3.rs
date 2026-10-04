use crate::error::StoreError;
use crate::models::fe3::{
    ApplicabilityBlob, AppxFamilyMetadata, CategoryInformation, Deployment, DigestEntry,
    PackageInstance, RelationshipGroup, Relationships, ResolvedFileLocation, UpdateProperties,
    UpdateRef,
};
use crate::services::display_catalog::ProgressEmitter;
use crate::utilities::helpers::string_to_package_type;
use log::{debug, trace, warn};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Endpoint constants
// ---------------------------------------------------------------------------

const FE3_DELIVERY: &str = "https://fe3.delivery.mp.microsoft.com/ClientWebService/client.asmx";
const FE3_DELIVERY_SECURED: &str =
    "https://fe3.delivery.mp.microsoft.com/ClientWebService/client.asmx/secured";

// ---------------------------------------------------------------------------
// Embedded XML templates
// ---------------------------------------------------------------------------

const GET_COOKIE_XML: &str = include_str!("../xml/get_cookie.xml");
const WUID_REQUEST_XML: &str = include_str!("../xml/wuid_request.xml");
const FE3_FILE_URL_XML: &str = include_str!("../xml/fe3_file_url.xml");

// ---------------------------------------------------------------------------
// Default MSA device token (from original StoreLib C# source)
// ---------------------------------------------------------------------------

const MSA_TOKEN: &str = "<Device>dAA9AEUAdwBBAHcAQQBzAE4AMwBCAEEAQQBVADEAYgB5AHMAZQBtAGIAZQBEAFYAQwArADMAZgBtADcAbwBXAHkASAA3AGIAbgBnAEcAWQBtAEEAQQBMAGoAbQBqAFYAVQB2AFEAYwA0AEsAVwBFAC8AYwBDAEwANQBYAGUANABnAHYAWABkAGkAegBHAGwAZABjADEAZAAvAFcAeQAvAHgASgBQAG4AVwBRAGUAYwBtAHYAbwBjAGkAZwA5AGoAZABwAE4AawBIAG0AYQBzAHAAVABKAEwARAArAFAAYwBBAFgAbQAvAFQAcAA3AEgAagBzAEYANAA0AEgAdABsAC8AMQBtAHUAcgAwAFMAdQBtAG8AMABZAGEAdgBqAFIANwArADQAcABoAC8AcwA4ADEANgBFAFkANQBNAFIAbQBnAFIAQwA2ADMAQwBSAEoAQQBVAHYAZgBzADQAaQB2AHgAYwB5AEwAbAA2AHoAOABlAHgAMABrAFgAOQBPAHcAYQB0ADEAdQBwAFMAOAAxAEgANgA4AEEASABzAEoAegBnAFQAQQBMAG8AbgBBADIAWQBBAEEAQQBpAGcANQBJADMAUQAvAFYASABLAHcANABBAEIAcQA5AFMAcQBhADEAQgA4AGsAVQAxAGEAbwBLAEEAdQA0AHYAbABWAG4AdwBWADMAUQB6AHMATgBtAEQAaQBqAGgANQBkAEcAcgBpADgAQQBlAEUARQBWAEcAbQBXAGgASQBCAE0AUAAyAEQAVwA0ADMAZABWAGkARABUAHoAVQB0AHQARQBMAEgAaABSAGYAcgBhAGIAWgBsAHQAQQBUAEUATABmAHMARQBGAFUAYQBRAFMASgB4ADUAeQBRADgAagBaAEUAZQAyAHgANABCADMAMQB2AEIAMgBqAC8AUgBLAGEAWQAvAHEAeQB0AHoANwBUAHYAdAB3AHQAagBzADYAUQBYAEIAZQA4AHMAZwBJAG8AOQBiADUAQQBCADcAOAAxAHMANgAvAGQAUwBFAHgATgBEAEQAYQBRAHoAQQBYAFAAWABCAFkAdQBYAFEARQBzAE8AegA4AHQAcgBpAGUATQBiAEIAZQBUAFkAOQBiAG8AQgBOAE8AaQBVADcATgBSAEYAOQAzAG8AVgArAFYAQQBiAGgAcAAwAHAAUgBQAFMAZQBmAEcARwBPAHEAdwBTAGcANwA3AHMAaAA5AEoASABNAHAARABNAFMAbgBrAHEAcgAyAGYARgBpAEMAUABrAHcAVgBvAHgANgBuAG4AeABGAEQAbwBXAC8AYQAxAHQAYQBaAHcAegB5AGwATAAxADIAdwB1AGIAbQA1AHUAbQBwAHEAeQBXAGMASwBSAGoAeQBoADIASgBUAEYASgBXADUAZwBYAEUASQA1AHAAOAAwAEcAdQAyAG4AeABMAFIATgB3AGkAdwByADcAVwBNAFIAQQBWAEsARgBXAE0AZQBSAHoAbAA5AFUAcQBnAC8AcABYAC8AdgBlAEwAdwBTAGsAMgBTAFMASABmAGEASwA2AGoAYQBvAFkAdQBuAFIARwByADgAbQBiAEUAbwBIAGwARgA2AEoAQwBhAGEAVABCAFgAQgBjAHYAdQBlAEMASgBvADkAOABoAFIAQQByAEcAdwA0ACsAUABIAGUAVABiAE4AUwBFAFgAWAB6AHYAWgA2AHUAVwA1AEUAQQBmAGQAWgBtAFMAOAA4AFYASgBjAFoAYQBGAEsANwB4AHgAZwAwAHcAbwBuADcAaAAwAHgAQwA2AFoAQgAwAGMAWQBqAEwAcgAvAEcAZQBPAHoAOQBHADQAUQBVAEgAOQBFAGsAeQAwAGQAeQBGAC8AcgBlAFUAMQBJAHkAaQBhAHAAcABoAE8AUAA4AFMAMgB0ADQAQgByAFAAWgBYAFQAdgBDADAA\
UAA3AHoATwArAGYARwBrAHgAVgBtACsAVQBmAFoAYgBRADUANQBzAHcARQA9ACYAcAA9AA==</Device>";

// ---------------------------------------------------------------------------
// FE3 handler
// ---------------------------------------------------------------------------
//
// Construct via [`FE3Handler::new`] for stored-callback progress
// (`fe3.linkReceived` per resolved URL via [`Self::get_file_urls`]); reach
// for the static `_with_progress` associated functions when the caller has
// its own enriched closure (e.g. `DisplayCatalogHandler::get_packages_for_product`
// attaches the owning moniker to each link event).

pub struct FE3Handler {
    pub(crate) client: reqwest::Client,
    /// Subscribe with `fe3.progress.set(Box::new(|e| ...))`.
    pub progress: ProgressEmitter,
}

impl FE3Handler {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            progress: ProgressEmitter::new(),
        }
    }

    /// Resolve direct download URLs for `(update_ids, revision_ids)`,
    /// firing one `fe3.linkReceived` event per resolved URL through
    /// [`Self::progress`]. `size` in the returned tuples is always `None`
    /// (FE3 only carries per-URL sizes in `SyncUpdates`).
    pub async fn get_file_urls(
        &self,
        update_ids: &[String],
        revision_ids: &[String],
        msa_token: Option<&str>,
    ) -> Result<Vec<(String, Option<i64>)>, StoreError> {
        let emitter = &self.progress;
        Self::get_file_urls_with_progress(
            update_ids,
            revision_ids,
            msa_token,
            &self.client,
            |idx, total, update_id, url, size| {
                emitter.emit_counter(
                    "fe3.linkReceived",
                    format!(
                        "uri={url} | size={} | updateId={update_id}",
                        size.map(|s| s.to_string()).unwrap_or_else(|| "?".into()),
                    ),
                    (idx + 1) as u32,
                    total as u32,
                );
            },
        )
        .await
    }

    // -----------------------------------------------------------------------
    // Cookie
    // -----------------------------------------------------------------------

    /// POST the GetCookie SOAP envelope to FE3 and return the `EncryptedData`
    /// value extracted from the response XML.
    pub async fn get_cookie(client: &reqwest::Client) -> Result<String, StoreError> {
        debug!("FE3: POST {FE3_DELIVERY} (GetCookie)");
        let response = client
            .post(FE3_DELIVERY)
            .header("Content-Type", "application/soap+xml; charset=utf-8")
            .body(GET_COOKIE_XML)
            .send()
            .await
            .map_err(StoreError::Http)?;

        let status = response.status();
        debug!("FE3 GetCookie response: HTTP {status}");

        let body = response.text().await.map_err(StoreError::Http)?;
        trace!("FE3 GetCookie body:\n{body}");

        let doc = roxmltree::Document::parse(&body).map_err(|e| StoreError::Xml(e.to_string()))?;

        let cookie = doc
            .descendants()
            .find(|n| n.tag_name().name() == "EncryptedData")
            .and_then(|n| n.text())
            .ok_or_else(|| {
                StoreError::Xml("EncryptedData node not found in cookie response".into())
            })?;

        debug!("FE3: cookie obtained ({} bytes)", cookie.len());
        Ok(cookie.to_owned())
    }

    // -----------------------------------------------------------------------
    // SyncUpdates
    // -----------------------------------------------------------------------

    /// POST a `SyncUpdates` request using a pre-obtained `cookie`. Returns the
    /// HTML-decoded SOAP response body.
    ///
    /// Most callers should use [`Self::sync_updates`], which fetches a cookie
    /// internally. This variant is for callers that need to emit a progress
    /// event between the two HTTP requests.
    pub async fn sync_updates_with_cookie(
        cookie: &str,
        wu_category_id: &str,
        msa_token: Option<&str>,
        client: &reqwest::Client,
    ) -> Result<String, StoreError> {
        let token = msa_token.unwrap_or(MSA_TOKEN);
        let body = WUID_REQUEST_XML
            .replace("{0}", cookie)
            .replace("{1}", wu_category_id)
            .replace("{2}", token);

        debug!("FE3: POST {FE3_DELIVERY} (SyncUpdates, WuCategoryId={wu_category_id})");
        let response = client
            .post(FE3_DELIVERY)
            .header("Content-Type", "application/soap+xml; charset=utf-8")
            .body(body)
            .send()
            .await
            .map_err(StoreError::Http)?;

        let status = response.status();
        debug!("FE3 SyncUpdates response: HTTP {status}");

        let raw = response.text().await.map_err(StoreError::Http)?;
        let decoded = html_decode(&raw);
        trace!("FE3 SyncUpdates body:\n{decoded}");
        Ok(decoded)
    }

    /// Fetch a FE3 cookie, then POST a `SyncUpdates` request for the given
    /// `wu_category_id`.  Returns the HTML-decoded SOAP response body.
    pub async fn sync_updates(
        wu_category_id: &str,
        msa_token: Option<&str>,
        client: &reqwest::Client,
    ) -> Result<String, StoreError> {
        let cookie = Self::get_cookie(client).await?;
        Self::sync_updates_with_cookie(&cookie, wu_category_id, msa_token, client).await
    }

    // -----------------------------------------------------------------------
    // Process update IDs
    // -----------------------------------------------------------------------

    /// Parse the raw `SyncUpdates` XML and extract `(update_ids, revision_ids)`.
    ///
    /// Only nodes whose XML fragment contains a `SecuredFragment` child are
    /// included, matching the logic from the original C# code.
    pub fn process_update_ids(xml: &str) -> Result<(Vec<String>, Vec<String>), StoreError> {
        let doc = roxmltree::Document::parse(xml).map_err(|e| StoreError::Xml(e.to_string()))?;

        let mut update_ids = Vec::new();
        let mut revision_ids = Vec::new();

        for node in doc.descendants() {
            if node.tag_name().name() != "SecuredFragment" {
                continue;
            }

            // SecuredFragment -> parent (Properties) -> parent (Xml/Update element)
            // -> direct UpdateIdentity child (element order is not identity).
            let identity = node.parent().and_then(|p| p.parent()).and_then(|gp| {
                gp.children()
                    .find(|child| child.tag_name().name() == "UpdateIdentity")
            });

            if let Some(identity) = identity {
                if let (Some(uid), Some(rev)) = (
                    identity.attribute("UpdateID"),
                    identity.attribute("RevisionNumber"),
                ) {
                    debug!("FE3: update ID={uid} revision={rev}");
                    update_ids.push(uid.to_owned());
                    revision_ids.push(rev.to_owned());
                }
            } else {
                warn!("FE3: SecuredFragment node has unexpected parent structure; skipping");
            }
        }

        debug!("FE3: process_update_ids found {} ID(s)", update_ids.len());
        Ok((update_ids, revision_ids))
    }

    // -----------------------------------------------------------------------
    // Package instances
    // -----------------------------------------------------------------------

    /// Parse `AppxMetadata` nodes from the `SyncUpdates` XML and build
    /// [`PackageInstance`] values (without resolved download URLs).
    ///
    /// Walks three structures and stitches them together by moniker:
    /// - `<AppxMetadata PackageMoniker="...">` — package identity + type +
    ///   `<ApplicabilityBlob>` JSON.
    /// - The rich `<Update>` block whose `<Files>/<File>` has
    ///   `InstallerSpecificIdentifier == PackageMoniker` — primary-binary
    ///   hash/size/timestamp plus the parent `<ExtendedProperties>` and
    ///   `<HandlerSpecificData>/<AppxPackageInstallData>` siblings.
    /// - Child digest tags (`AdditionalDigest`, `PiecesHashDigest`,
    ///   `BlockMapDigest`) under the primary `<File>`.
    pub async fn get_package_instances(xml: &str) -> Result<Vec<PackageInstance>, StoreError> {
        let doc = roxmltree::Document::parse(xml).map_err(|e| StoreError::Xml(e.to_string()))?;

        // First pass: index every <File> by its InstallerSpecificIdentifier
        // (which equals PackageMoniker). Blockmap entries (Abm_*.cab) have
        // no InstallerSpecificIdentifier so they don't enter the index; the
        // primary binary wins by construction.
        let mut primary_file_by_moniker: std::collections::HashMap<String, roxmltree::Node> =
            std::collections::HashMap::new();
        for node in doc.descendants() {
            if node.tag_name().name() != "File" {
                continue;
            }
            if let Some(moniker) = node.attribute("InstallerSpecificIdentifier") {
                primary_file_by_moniker
                    .entry(moniker.to_owned())
                    .or_insert(node);
            }
        }

        // Index every <AppxFamilyMetadata> by its Name. Family metadata lives
        // in separate family/category updates (not as a sibling of the
        // per-package <AppxMetadata>), and is keyed to packages by the moniker
        // prefix — everything before the first '_'.
        let mut family_meta_by_name: std::collections::HashMap<String, AppxFamilyMetadata> =
            std::collections::HashMap::new();
        for node in doc.descendants() {
            if node.tag_name().name() != "AppxFamilyMetadata" {
                continue;
            }
            if let Some(name) = node.attribute("Name") {
                family_meta_by_name
                    .entry(name.to_owned())
                    .or_insert_with(|| AppxFamilyMetadata {
                        name: Some(name.to_owned()),
                        publisher: node.attribute("Publisher").map(String::from),
                        legacy_mobile_product_id: node
                            .attribute("LegacyMobileProductId")
                            .map(String::from),
                    });
            }
        }

        let mut instances = Vec::new();
        for node in doc.descendants() {
            if node.tag_name().name() != "AppxMetadata" {
                continue;
            }

            // Must have at least 3 attributes (PackageMoniker, PackageType, ...)
            let attrs: Vec<_> = node.attributes().collect();
            if attrs.len() < 3 {
                continue;
            }

            let moniker = match node.attribute("PackageMoniker") {
                Some(v) => v.to_owned(),
                None => continue,
            };
            let pkg_type_str = node.attribute("PackageType").unwrap_or("");
            let pkg_type = string_to_package_type(pkg_type_str);
            let is_appx_bundle = node
                .attribute("IsAppxBundle")
                .and_then(|v| v.parse::<bool>().ok());

            debug!("FE3: package instance moniker={moniker} type={pkg_type_str}");

            // First child text node carries the ApplicabilityBlob JSON.
            let blob: Option<ApplicabilityBlob> =
                node.first_child().and_then(|c| c.text()).and_then(|t| {
                    trace!("FE3: ApplicabilityBlob JSON: {t}");
                    serde_json::from_str(t).ok()
                });

            // ----- Per-binary <File> attributes + child digests ---------
            let primary_file = primary_file_by_moniker.get(&moniker).copied();
            let file_str = |name: &str| {
                primary_file
                    .and_then(|f| f.attribute(name))
                    .map(String::from)
            };
            let file_i64 = |name: &str| {
                primary_file
                    .and_then(|f| f.attribute(name))
                    .and_then(|s| s.parse::<i64>().ok())
            };

            let file_name = primary_file.and_then(|f| f.attribute("FileName"));
            let digest = file_str("Digest");
            let digest_algorithm = file_str("DigestAlgorithm");
            let file_size_attr = file_i64("Size");
            let modified = file_str("Modified");
            let patching_type = file_str("PatchingType");

            let mut additional_digests: Vec<DigestEntry> = Vec::new();
            let mut pieces_hash_digest: Option<DigestEntry> = None;
            let mut block_map_digest: Option<DigestEntry> = None;
            if let Some(file) = primary_file {
                for child in file.children() {
                    if !child.is_element() {
                        continue;
                    }
                    let alg = child.attribute("Algorithm").unwrap_or("").to_string();
                    let val = child.text().unwrap_or("").trim().to_string();
                    if val.is_empty() {
                        continue;
                    }
                    let entry = DigestEntry {
                        algorithm: alg,
                        value: val,
                    };
                    match child.tag_name().name() {
                        "AdditionalDigest" => additional_digests.push(entry),
                        "PiecesHashDigest" => pieces_hash_digest = Some(entry),
                        "BlockMapDigest" => block_map_digest = Some(entry),
                        _ => {}
                    }
                }
            }

            // ----- Walk up to the owning <Update>/<Xml> to pick up
            //       ExtendedProperties + AppxPackageInstallData siblings.
            // File -> Files -> Xml -> Update
            let update_xml = primary_file
                .and_then(|f| f.parent())
                .and_then(|files| files.parent());

            let ext_props = update_xml.and_then(|xml_node| {
                xml_node
                    .children()
                    .find(|c| c.tag_name().name() == "ExtendedProperties")
            });

            let ext_str = |name: &str| ext_props.and_then(|n| n.attribute(name)).map(String::from);
            let ext_bool = |name: &str| {
                ext_props
                    .and_then(|n| n.attribute(name))
                    .and_then(|v| v.parse::<bool>().ok())
            };
            let ext_i64 = |name: &str| {
                ext_props
                    .and_then(|n| n.attribute(name))
                    .and_then(|s| s.parse::<i64>().ok())
            };

            let handler = ext_str("Handler");
            let is_appx_framework = ext_bool("IsAppxFramework");
            let max_download_size = ext_i64("MaxDownloadSize");
            let min_download_size = ext_i64("MinDownloadSize");
            let package_content_id = ext_str("PackageContentId");
            let package_identity_name = ext_str("PackageIdentityName");
            let creation_date = ext_str("CreationDate");
            let content_type = ext_str("ContentType");
            let mandatory_version = ext_str("MandatoryVersion");
            let mandatory_date = ext_str("MandatoryDate");
            let default_properties_language = ext_str("DefaultPropertiesLanguage");
            let from_store_service = ext_bool("FromStoreService");
            let legacy_mobile_product_id = ext_str("LegacyMobileProductId");

            // ----- <AppxPackageInstallData> + <HandlerSpecificData> -------
            let appx_install_data = update_xml.and_then(|xml_node| {
                xml_node
                    .descendants()
                    .find(|c| c.tag_name().name() == "AppxPackageInstallData")
            });
            let main_package = appx_install_data
                .and_then(|n| n.attribute("MainPackage"))
                .and_then(|v| v.parse::<bool>().ok());
            let package_file_name = appx_install_data
                .and_then(|n| n.attribute("PackageFileName"))
                .map(String::from);

            let handler_specific = update_xml.and_then(|xml_node| {
                xml_node
                    .descendants()
                    .find(|c| c.tag_name().name() == "HandlerSpecificData")
            });
            let handler_type = handler_specific
                .and_then(|n| n.attribute("type"))
                .map(String::from);
            let category_information = handler_specific
                .and_then(|h| {
                    h.children()
                        .find(|c| c.tag_name().name() == "CategoryInformation")
                })
                .map(|c| CategoryInformation {
                    category_type: c.attribute("CategoryType").map(String::from),
                    display_order: c.attribute("DisplayOrder").and_then(|v| v.parse().ok()),
                    exclude_by_default: c
                        .attribute("ExcludeByDefault")
                        .and_then(|v| v.parse().ok()),
                    excluded_by_default: c
                        .attribute("ExcludedByDefault")
                        .and_then(|v| v.parse().ok()),
                    prohibits_subcategories: c
                        .attribute("ProhibitsSubcategories")
                        .and_then(|v| v.parse().ok()),
                    prohibits_updates: c.attribute("ProhibitsUpdates").and_then(|v| v.parse().ok()),
                });

            let installer_specific_identifier = primary_file
                .and_then(|f| f.attribute("InstallerSpecificIdentifier"))
                .map(String::from);

            // ----- NewUpdates <Xml>: own identity, <Properties>,
            //       <Relationships>, raw <ApplicabilityRules> ---------------
            let new_xml = node.ancestors().find(|a| a.tag_name().name() == "Xml");
            let own_identity = new_xml.and_then(|x| {
                x.children()
                    .find(|c| c.tag_name().name() == "UpdateIdentity")
            });
            let update_id = own_identity
                .and_then(|n| n.attribute("UpdateID"))
                .unwrap_or_default()
                .to_owned();
            let revision_number = own_identity
                .and_then(|n| n.attribute("RevisionNumber"))
                .map(String::from);

            let update_props_node =
                new_xml.and_then(|x| x.children().find(|c| c.tag_name().name() == "Properties"));
            let update_properties = update_props_node.map(|p| UpdateProperties {
                apply_package_rank: p.attribute("ApplyPackageRank").map(String::from),
                explicitly_deployable: p.attribute("ExplicitlyDeployable").map(String::from),
                is_appx_framework: p.attribute("IsAppxFramework").and_then(|v| v.parse().ok()),
                package_rank: p.attribute("PackageRank").and_then(|v| v.parse().ok()),
                per_user: p.attribute("PerUser").map(String::from),
                update_type: p.attribute("UpdateType").map(String::from),
            });

            // Full <Relationships> — prerequisites + bundled updates,
            // preserving <AtLeastOne IsCategory> grouping and per-ref
            // RevisionNumber. Flat Vec<String> views are derived for
            // convenience (and backward compatibility).
            let mut relationships = Relationships::default();
            if let Some(rel) = new_xml.and_then(|x| {
                x.children()
                    .find(|c| c.tag_name().name() == "Relationships")
            }) {
                for child in rel.children() {
                    match child.tag_name().name() {
                        "Prerequisites" => {
                            relationships.prerequisites = parse_relationship_groups(child)
                        }
                        "BundledUpdates" => {
                            relationships.bundled_updates = parse_relationship_groups(child)
                        }
                        _ => {}
                    }
                }
            }
            let flatten = |groups: &[RelationshipGroup]| -> Vec<String> {
                groups
                    .iter()
                    .flat_map(|g| g.updates.iter())
                    .map(|u| u.update_id.clone())
                    .collect()
            };
            let prerequisites = flatten(&relationships.prerequisites);
            let bundled_updates = flatten(&relationships.bundled_updates);
            debug!(
                "FE3: package {moniker}: {} prerequisite id(s), {} bundled id(s)",
                prerequisites.len(),
                bundled_updates.len(),
            );

            // Raw subtrees preserved verbatim so their nested rules aren't lost.
            let applicability_rules_xml = new_xml
                .and_then(|x| {
                    x.children()
                        .find(|c| c.tag_name().name() == "ApplicabilityRules")
                })
                .map(|n| xml[n.range()].to_string());
            let installation_behavior_xml = ext_props
                .and_then(|e| {
                    e.children()
                        .find(|c| c.tag_name().name() == "InstallationBehavior")
                })
                .filter(|n| n.has_children() || n.attributes().next().is_some())
                .map(|n| xml[n.range()].to_string());

            // ----- <UpdateInfo> envelope: numeric ID, IsLeaf, IsShared,
            //       <Deployment> --------------------------------------------
            let update_info = node
                .ancestors()
                .find(|a| a.tag_name().name() == "UpdateInfo");
            let child_text = |parent: Option<roxmltree::Node>, name: &str| {
                parent
                    .and_then(|p| p.children().find(|c| c.tag_name().name() == name))
                    .and_then(|c| c.text())
                    .map(|t| t.trim().to_string())
            };
            let update_info_id = child_text(update_info, "ID");
            let is_leaf = child_text(update_info, "IsLeaf").and_then(|s| s.parse::<bool>().ok());
            let is_shared =
                child_text(update_info, "IsShared").and_then(|s| s.parse::<bool>().ok());

            let deployment = update_info
                .and_then(|u| u.children().find(|c| c.tag_name().name() == "Deployment"))
                .map(|d| {
                    let dt = |name: &str| {
                        d.children()
                            .find(|c| c.tag_name().name() == name)
                            .and_then(|c| c.text())
                            .map(|t| t.trim().to_string())
                    };
                    Deployment {
                        id: dt("ID"),
                        action: dt("Action"),
                        is_assigned: dt("IsAssigned"),
                        last_change_time: dt("LastChangeTime"),
                        auto_select: dt("AutoSelect"),
                        auto_download: dt("AutoDownload"),
                        supersedence_behavior: dt("SupersedenceBehavior"),
                        priority: dt("Priority"),
                        handler_specific_action: dt("HandlerSpecificAction"),
                        flight_id: dt("FlightId"),
                    }
                });

            // ----- <AppxFamilyMetadata> — lives in a separate family update;
            //       associate by moniker family prefix (before the first '_').
            let family_metadata = moniker
                .split('_')
                .next()
                .and_then(|fam| family_meta_by_name.get(fam))
                .cloned();

            // ----- Catch-all: any attribute not mapped to a typed field ---
            let mut extra_attributes: BTreeMap<String, String> = BTreeMap::new();
            collect_extra(
                node,
                &["PackageMoniker", "PackageType", "IsAppxBundle"],
                &mut extra_attributes,
            );
            if let Some(f) = primary_file {
                collect_extra(
                    f,
                    &[
                        "Digest",
                        "DigestAlgorithm",
                        "FileName",
                        "InstallerSpecificIdentifier",
                        "Modified",
                        "PatchingType",
                        "Size",
                    ],
                    &mut extra_attributes,
                );
            }
            if let Some(e) = ext_props {
                collect_extra(
                    e,
                    &[
                        "ContentType",
                        "CreationDate",
                        "DefaultPropertiesLanguage",
                        "FromStoreService",
                        "Handler",
                        "IsAppxFramework",
                        "LegacyMobileProductId",
                        "MandatoryDate",
                        "MandatoryVersion",
                        "MaxDownloadSize",
                        "MinDownloadSize",
                        "PackageContentId",
                        "PackageIdentityName",
                    ],
                    &mut extra_attributes,
                );
            }
            if let Some(n) = appx_install_data {
                collect_extra(
                    n,
                    &["MainPackage", "PackageFileName"],
                    &mut extra_attributes,
                );
            }
            if let Some(n) = update_props_node {
                collect_extra(
                    n,
                    &[
                        "ApplyPackageRank",
                        "ExplicitlyDeployable",
                        "IsAppxFramework",
                        "PackageRank",
                        "PerUser",
                        "UpdateType",
                    ],
                    &mut extra_attributes,
                );
            }
            if let Some(n) = handler_specific {
                collect_extra(n, &["type"], &mut extra_attributes);
            }
            if let Some(n) = own_identity {
                collect_extra(n, &["UpdateID", "RevisionNumber"], &mut extra_attributes);
            }

            // Ready-to-use lowercase-hex digests of the exact bytes FE3
            // serves, decoded from the base64 <File Digest> / <AdditionalDigest>
            // values above so consumers don't have to decode + algorithm-match.
            let sha1 = PackageInstance::select_digest_hex(
                digest.as_deref(),
                digest_algorithm.as_deref(),
                &additional_digests,
                "SHA1",
            );
            let sha256 = PackageInstance::select_digest_hex(
                digest.as_deref(),
                digest_algorithm.as_deref(),
                &additional_digests,
                "SHA256",
            );

            // file_size: prefer <File Size>, fall back to
            // <ExtendedProperties MaxDownloadSize>. (DCat is the final
            // fallback, applied by display_catalog.)
            let file_size = file_size_attr.or(max_download_size);

            let readable_file_name = PackageInstance::build_readable_file_name(&moniker, file_name);

            instances.push(PackageInstance {
                package_moniker: moniker,
                package_uri: None,
                package_type: pkg_type,
                applicability_blob: blob,
                update_id,
                file_size,
                file_name: file_name.map(String::from),
                readable_file_name,

                is_appx_bundle,

                digest,
                digest_algorithm,
                modified,
                patching_type,
                additional_digests,
                pieces_hash_digest,
                block_map_digest,
                sha1,
                sha256,

                handler,
                is_appx_framework,
                max_download_size,
                min_download_size,
                package_content_id,
                package_identity_name,
                creation_date,
                content_type,
                mandatory_version,
                mandatory_date,
                default_properties_language,
                from_store_service,
                legacy_mobile_product_id,

                main_package,

                revision_number,
                update_info_id,
                is_leaf,
                is_shared,

                installer_specific_identifier,
                package_file_name,
                handler_type,

                prerequisites,
                bundled_updates,
                relationships,

                update_properties,
                family_metadata,
                category_information,
                deployment,
                applicability_rules_xml,
                installation_behavior_xml,

                extra_attributes,

                all_file_locations: Vec::new(),
            });
        }

        Ok(instances)
    }

    // -----------------------------------------------------------------------
    // File URLs
    // -----------------------------------------------------------------------

    /// For each `(update_id, revision_id)` pair, POST a
    /// `GetExtendedUpdateInfo2` SOAP request to FE3 and fire `on_url` once
    /// per successfully-resolved (non-blockmap) URL **as soon as the SOAP
    /// response is parsed**, before the next request goes out. Returns
    /// `Vec<(url, size_or_none)>` keyed parallel to the inputs.
    ///
    /// Use this static when you need a custom per-link closure (e.g. the
    /// DCat path attaches the owning package's moniker). For the common
    /// case where `fe3.linkReceived` events should flow through a
    /// pre-installed handler-level callback, use the instance method
    /// [`Self::get_file_urls`] instead.
    ///
    /// Callback args: `(request_idx, request_total, update_id, url, size)`.
    /// `size` is always `None` — FE3 does not return per-URL sizes in
    /// `GetExtendedUpdateInfo2` responses (the size lives on `<File Size>`
    /// in SyncUpdates). Kept in the signature for source compatibility.
    pub async fn get_file_urls_with_progress<F>(
        update_ids: &[String],
        revision_ids: &[String],
        msa_token: Option<&str>,
        client: &reqwest::Client,
        on_url: F,
    ) -> Result<Vec<(String, Option<i64>)>, StoreError>
    where
        F: Fn(usize, usize, &str, &str, Option<i64>),
    {
        let per_update = Self::get_file_locations_with_progress(
            update_ids,
            revision_ids,
            msa_token,
            client,
            |idx, total, update_id, loc| {
                on_url(idx, total, update_id, &loc.url, None);
            },
        )
        .await?;

        // Match the legacy single-URL-per-update contract: pick the first
        // location for each update_id (the primary binary; blockmaps are
        // filtered upstream by digest matching).
        Ok(per_update
            .into_iter()
            .map(|locs| {
                locs.into_iter()
                    .next()
                    .map(|l| (l.url, None))
                    .unwrap_or_default()
            })
            .filter(|(u, _)| !u.is_empty())
            .collect())
    }

    /// Rich variant of [`Self::get_file_urls_with_progress`] that returns
    /// *every* `<FileLocation>` for each update — primary binary, blockmap,
    /// and signed alternative — along with the per-URL `<FileDigest>` so
    /// callers can match a URL back to its `<File>` entry. Fires `on_loc`
    /// once per resolved `<FileLocation>` as soon as the SOAP response is
    /// parsed.
    ///
    /// Returns a `Vec` indexed parallel to `update_ids`: each inner `Vec`
    /// is the file locations FE3 returned for that update, in document
    /// order. Blockmaps are NOT filtered here — that's a job for the
    /// caller (match `digest` against `PackageInstance::digest` to pick
    /// the binary, or against `PackageInstance::block_map_digest` to pick
    /// the blockmap).
    ///
    /// Callback args: `(request_idx, request_total, update_id, &location)`.
    pub async fn get_file_locations_with_progress<F>(
        update_ids: &[String],
        revision_ids: &[String],
        msa_token: Option<&str>,
        client: &reqwest::Client,
        on_loc: F,
    ) -> Result<Vec<Vec<ResolvedFileLocation>>, StoreError>
    where
        F: Fn(usize, usize, &str, &ResolvedFileLocation),
    {
        let token = msa_token.unwrap_or(MSA_TOKEN);
        let total = update_ids.len();
        let mut all_per_update: Vec<Vec<ResolvedFileLocation>> = Vec::with_capacity(total);

        for (i, update_id) in update_ids.iter().enumerate() {
            let revision_id = match revision_ids.get(i) {
                Some(r) => r.as_str(),
                None => {
                    all_per_update.push(Vec::new());
                    continue;
                }
            };

            let body = FE3_FILE_URL_XML
                .replace("{0}", update_id)
                .replace("{1}", revision_id)
                .replace("{2}", token);

            debug!("FE3: POST {FE3_DELIVERY_SECURED} (GetExtendedUpdateInfo2, UpdateID={update_id} RevisionID={revision_id})");
            let response = client
                .post(FE3_DELIVERY_SECURED)
                .header("Content-Type", "application/soap+xml; charset=utf-8")
                .body(body)
                .send()
                .await
                .map_err(StoreError::Http)?;

            let status = response.status();
            debug!("FE3 GetExtendedUpdateInfo2 response: HTTP {status}");

            let raw = response.text().await.map_err(StoreError::Http)?;
            debug!("FE3 GetExtendedUpdateInfo2 body:\n{raw}");

            let doc = match roxmltree::Document::parse(&raw) {
                Ok(d) => d,
                Err(e) => return Err(StoreError::Xml(e.to_string())),
            };

            let mut locs_for_update: Vec<ResolvedFileLocation> = Vec::new();
            for file_loc in doc.descendants() {
                if file_loc.tag_name().name() != "FileLocation" {
                    continue;
                }
                let mut url_opt: Option<String> = None;
                let mut digest_opt: Option<String> = None;
                for child in file_loc.children() {
                    match child.tag_name().name() {
                        "Url" => {
                            if let Some(text) = child.text() {
                                debug!("FE3: URL resolved: {text}");
                                url_opt = Some(text.to_owned());
                            }
                        }
                        "FileDigest" => {
                            digest_opt = child.text().map(|t| t.trim().to_owned());
                            trace!("FE3: FileDigest={digest_opt:?}");
                        }
                        _ => {}
                    }
                }
                if let Some(url) = url_opt {
                    let loc = ResolvedFileLocation {
                        url,
                        digest: digest_opt,
                    };
                    on_loc(i, total, update_id, &loc);
                    locs_for_update.push(loc);
                }
            }
            all_per_update.push(locs_for_update);
        }

        Ok(all_per_update)
    }
}

// ---------------------------------------------------------------------------
// Relationship / attribute parsing helpers
// ---------------------------------------------------------------------------

/// Build an [`UpdateRef`] from an `<UpdateIdentity>` node, keeping both the
/// `UpdateID` and the `RevisionNumber`.
fn update_ref_from(n: roxmltree::Node) -> UpdateRef {
    UpdateRef {
        update_id: n.attribute("UpdateID").unwrap_or("").to_string(),
        revision_number: n.attribute("RevisionNumber").map(String::from),
    }
}

/// Parse a `<Prerequisites>` or `<BundledUpdates>` node into
/// [`RelationshipGroup`]s, preserving `<AtLeastOne IsCategory="…">` grouping
/// and any bare `<UpdateIdentity>` children.
fn parse_relationship_groups(parent: roxmltree::Node) -> Vec<RelationshipGroup> {
    let mut groups = Vec::new();
    for child in parent.children() {
        if !child.is_element() {
            continue;
        }
        match child.tag_name().name() {
            "AtLeastOne" => groups.push(RelationshipGroup {
                is_category: child.attribute("IsCategory").and_then(|v| v.parse().ok()),
                updates: child
                    .descendants()
                    .filter(|d| d.tag_name().name() == "UpdateIdentity")
                    .map(update_ref_from)
                    .collect(),
            }),
            "UpdateIdentity" => groups.push(RelationshipGroup {
                is_category: None,
                updates: vec![update_ref_from(child)],
            }),
            _ => {}
        }
    }
    groups
}

/// Record every attribute on `node` whose name is not in `known` into `map`,
/// keyed `"<Element>@<Attr>"`. This guarantees no attribute is ever silently
/// dropped — including fields Microsoft may add in the future.
fn collect_extra(node: roxmltree::Node, known: &[&str], map: &mut BTreeMap<String, String>) {
    let tag = node.tag_name().name();
    for a in node.attributes() {
        if !known.contains(&a.name()) {
            map.insert(format!("{tag}@{}", a.name()), a.value().to_string());
        }
    }
}

// ---------------------------------------------------------------------------
// HTML entity decoder
// ---------------------------------------------------------------------------

/// Minimal HTML entity decoder covering the entities used in SOAP responses.
pub(crate) fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_decode_all_entities() {
        assert_eq!(html_decode("a &amp; b"), "a & b");
        assert_eq!(html_decode("&lt;tag&gt;"), "<tag>");
        assert_eq!(html_decode("say &quot;hello&quot;"), "say \"hello\"");
        assert_eq!(html_decode("it&apos;s"), "it's");
    }

    #[test]
    fn html_decode_no_entities() {
        assert_eq!(html_decode("plain text"), "plain text");
    }

    #[test]
    fn html_decode_chained_entities() {
        // Replacements are sequential: &amp;lt; -> &lt; -> <
        assert_eq!(html_decode("&amp;lt;"), "<");
    }

    #[test]
    fn process_update_ids_empty_xml() {
        let xml = r#"<?xml version="1.0"?><root></root>"#;
        let (ids, revs) = FE3Handler::process_update_ids(xml).unwrap();
        assert!(ids.is_empty());
        assert!(revs.is_empty());
    }

    #[test]
    fn process_update_ids_parses_secured_fragment() {
        // Minimal SyncUpdates-style XML with one SecuredFragment node.
        let xml = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
  <s:Body>
    <SyncUpdatesResult>
      <NewUpdates>
        <UpdateInfo>
          <ID>1</ID>
          <Xml>
            <UpdateIdentity UpdateID="abc-123" RevisionNumber="200"/>
            <Properties>
              <SecuredFragment/>
            </Properties>
          </Xml>
        </UpdateInfo>
      </NewUpdates>
    </SyncUpdatesResult>
  </s:Body>
</s:Envelope>"#;
        let (ids, revs) = FE3Handler::process_update_ids(xml).unwrap();
        assert_eq!(ids, vec!["abc-123"]);
        assert_eq!(revs, vec!["200"]);
    }

    #[test]
    fn process_update_ids_skips_nodes_without_secured_fragment() {
        let xml = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
  <s:Body>
    <SyncUpdatesResult>
      <NewUpdates>
        <UpdateInfo>
          <ID>1</ID>
          <Xml>
            <UpdateIdentity UpdateID="no-fragment" RevisionNumber="1"/>
            <Properties/>
          </Xml>
        </UpdateInfo>
      </NewUpdates>
    </SyncUpdatesResult>
  </s:Body>
</s:Envelope>"#;
        let (ids, _) = FE3Handler::process_update_ids(xml).unwrap();
        assert!(ids.is_empty());
    }

    #[test]
    fn process_update_ids_multiple() {
        let xml = r#"<?xml version="1.0"?>
<root>
  <Update>
    <UpdateIdentity UpdateID="id-1" RevisionNumber="10"/>
    <Properties>
      <SecuredFragment/>
    </Properties>
  </Update>
  <Update>
    <UpdateIdentity UpdateID="id-2" RevisionNumber="20"/>
    <Properties>
      <SecuredFragment/>
    </Properties>
  </Update>
</root>"#;
        let (ids, revs) = FE3Handler::process_update_ids(xml).unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], "id-1");
        assert_eq!(revs[0], "10");
        assert_eq!(ids[1], "id-2");
        assert_eq!(revs[1], "20");
    }

    #[test]
    fn process_update_ids_invalid_xml_returns_error() {
        let result = FE3Handler::process_update_ids("not xml at all <<<");
        assert!(result.is_err());
    }

    // -- get_package_instances: prerequisites (dependency graph) -----------

    /// Minimal SyncUpdates fragment mirroring the real wire shape: an
    /// `AppxMetadata` nested under `Xml/ApplicabilityRules/Metadata/
    /// AppxPackageMetadata`, with a sibling `<Relationships><Prerequisites>`
    /// under the same `<Xml>` carrying two prerequisite category IDs.
    const SYNC_WITH_PREREQS: &str = r#"<?xml version="1.0"?>
<root>
  <UpdateInfo>
    <ID>1</ID>
    <Xml>
      <UpdateIdentity UpdateID="own-app-update-id" RevisionNumber="1"/>
      <Properties><SecuredFragment/></Properties>
      <Relationships>
        <Prerequisites>
          <AtLeastOne IsCategory="true">
            <UpdateIdentity UpdateID="cat-framework-1"/>
          </AtLeastOne>
          <AtLeastOne IsCategory="true">
            <UpdateIdentity UpdateID="cat-framework-2"/>
          </AtLeastOne>
        </Prerequisites>
      </Relationships>
      <ApplicabilityRules>
        <Metadata>
          <AppxPackageMetadata>
            <AppxMetadata PackageMoniker="App.Pkg_1.0_x64__hash" PackageType="AppX" IsAppxBundle="false">{"content.packageId":"abc"}</AppxMetadata>
          </AppxPackageMetadata>
        </Metadata>
      </ApplicabilityRules>
    </Xml>
  </UpdateInfo>
</root>"#;

    #[tokio::test]
    async fn package_identity_comes_from_its_own_update() {
        let xml = r#"<root>
          <UpdateInfo><Xml>
            <UpdateIdentity UpdateID="non-package" RevisionNumber="1"/>
            <Properties><SecuredFragment/></Properties>
          </Xml></UpdateInfo>
          <UpdateInfo><Xml>
            <UpdateIdentity UpdateID="package-owner" RevisionNumber="7"/>
            <Properties><SecuredFragment/></Properties>
            <Metadata>
              <AppxMetadata PackageMoniker="App.A" PackageType="AppX" IsAppxBundle="false">{}</AppxMetadata>
              <AppxMetadata PackageMoniker="App.B" PackageType="AppX" IsAppxBundle="false">{}</AppxMetadata>
            </Metadata>
          </Xml></UpdateInfo>
        </root>"#;
        let packages = FE3Handler::get_package_instances(xml).await.unwrap();
        assert_eq!(packages.len(), 2);
        for package in packages {
            assert_eq!(package.update_id, "package-owner");
            assert_eq!(package.revision_number.as_deref(), Some("7"));
        }
    }

    #[test]
    fn process_update_ids_does_not_require_identity_to_be_first_child() {
        let xml = r#"<root><Xml>
          <Properties><SecuredFragment/></Properties>
          <UpdateIdentity UpdateID="owner" RevisionNumber="7"/>
        </Xml></root>"#;
        let (ids, revisions) = FE3Handler::process_update_ids(xml).unwrap();
        assert_eq!(ids, ["owner"]);
        assert_eq!(revisions, ["7"]);
    }

    #[tokio::test]
    async fn get_package_instances_extracts_prerequisites() {
        let instances = FE3Handler::get_package_instances(SYNC_WITH_PREREQS)
            .await
            .unwrap();
        assert_eq!(instances.len(), 1);
        let inst = &instances[0];
        assert_eq!(inst.package_moniker, "App.Pkg_1.0_x64__hash");
        // Both prerequisite *category* IDs are captured, in document order.
        assert_eq!(
            inst.prerequisites,
            vec!["cat-framework-1".to_string(), "cat-framework-2".to_string()],
        );
        // The package's own UpdateIdentity must NOT leak into prerequisites.
        assert!(!inst
            .prerequisites
            .contains(&"own-app-update-id".to_string()));
    }

    /// Full-fidelity fixture: every element/attribute the real SyncUpdates
    /// response carries, so the parser is proven to drop nothing.
    const SYNC_FULL: &str = r#"<?xml version="1.0"?>
<root>
  <NewUpdates>
    <UpdateInfo>
      <ID>329883205</ID>
      <Deployment>
        <ID>578536343</ID>
        <Action>Install</Action>
        <IsAssigned>true</IsAssigned>
        <LastChangeTime>2026-04-02</LastChangeTime>
        <AutoSelect>1</AutoSelect>
        <AutoDownload>2</AutoDownload>
        <SupersedenceBehavior>0</SupersedenceBehavior>
        <Priority>0</Priority>
        <HandlerSpecificAction>0</HandlerSpecificAction>
        <FlightId>ABC</FlightId>
      </Deployment>
      <IsLeaf>true</IsLeaf>
      <IsShared>false</IsShared>
      <Xml>
        <UpdateIdentity UpdateID="own-update" RevisionNumber="42"/>
        <Properties UpdateType="Software" PackageRank="1000" PerUser="false" IsAppxFramework="true" ApplyPackageRank="true" ExplicitlyDeployable="true"><SecuredFragment/></Properties>
        <Relationships>
          <Prerequisites>
            <AtLeastOne IsCategory="true"><UpdateIdentity UpdateID="cat-1"/></AtLeastOne>
            <UpdateIdentity UpdateID="direct-prereq"/>
          </Prerequisites>
          <BundledUpdates>
            <UpdateIdentity UpdateID="child-1" RevisionNumber="7"/>
            <UpdateIdentity UpdateID="child-2"/>
          </BundledUpdates>
        </Relationships>
        <ApplicabilityRules><IsInstalled><True/></IsInstalled></ApplicabilityRules>
        <Metadata>
          <AppxPackageMetadata>
            <AppxFamilyMetadata Name="App.Pkg" Publisher="CN=Test" LegacyMobileProductId="legacy-1"/>
            <AppxMetadata PackageMoniker="App.Pkg_1.0_x64__hash" PackageType="AppX" IsAppxBundle="false" FutureAttr="surprise">{"content.packageId":"abc"}</AppxMetadata>
          </AppxPackageMetadata>
        </Metadata>
      </Xml>
    </UpdateInfo>
  </NewUpdates>
  <ExtendedUpdateInfo>
    <Updates>
      <Update>
        <ID>329883205</ID>
        <Xml>
          <ExtendedProperties Handler="appx" IsAppxFramework="true" MaxDownloadSize="123" PackageIdentityName="App.Pkg">
            <InstallationBehavior RebootRequired="false"/>
          </ExtendedProperties>
          <Files>
            <File FileName="guid.appxbundle" InstallerSpecificIdentifier="App.Pkg_1.0_x64__hash" Digest="abc" DigestAlgorithm="SHA1" Size="123" Modified="2026-01-01"/>
          </Files>
          <HandlerSpecificData type="appx:AppxInstaller">
            <AppxPackageInstallData MainPackage="true" PackageFileName="guid.appxbundle"/>
            <CategoryInformation CategoryType="App" DisplayOrder="5" ExcludeByDefault="false" ExcludedByDefault="false" ProhibitsSubcategories="true" ProhibitsUpdates="false"/>
          </HandlerSpecificData>
        </Xml>
      </Update>
    </Updates>
  </ExtendedUpdateInfo>
</root>"#;

    #[tokio::test]
    async fn get_package_instances_captures_every_field() {
        let pkgs = FE3Handler::get_package_instances(SYNC_FULL).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        let p = &pkgs[0];

        // Relationships — full structure + flat views, nothing dropped.
        assert_eq!(p.prerequisites, vec!["cat-1", "direct-prereq"]);
        assert_eq!(p.bundled_updates, vec!["child-1", "child-2"]);
        assert_eq!(p.relationships.prerequisites.len(), 2);
        assert_eq!(p.relationships.prerequisites[0].is_category, Some(true));
        assert_eq!(
            p.relationships.prerequisites[0].updates[0].update_id,
            "cat-1"
        );
        assert_eq!(p.relationships.prerequisites[1].is_category, None);
        assert_eq!(
            p.relationships.bundled_updates[0].updates[0].update_id,
            "child-1"
        );
        assert_eq!(
            p.relationships.bundled_updates[0].updates[0]
                .revision_number
                .as_deref(),
            Some("7"),
        );

        // Update identity / envelope.
        assert_eq!(p.revision_number.as_deref(), Some("42"));
        assert_eq!(p.update_info_id.as_deref(), Some("329883205"));
        assert_eq!(p.is_leaf, Some(true));
        assert_eq!(p.is_shared, Some(false));

        // File / install data previously dropped.
        assert_eq!(
            p.installer_specific_identifier.as_deref(),
            Some("App.Pkg_1.0_x64__hash")
        );
        assert_eq!(p.package_file_name.as_deref(), Some("guid.appxbundle"));
        assert_eq!(p.handler_type.as_deref(), Some("appx:AppxInstaller"));

        // Rich blocks previously dropped entirely.
        let up = p.update_properties.as_ref().unwrap();
        assert_eq!(up.update_type.as_deref(), Some("Software"));
        assert_eq!(up.package_rank, Some(1000));
        assert_eq!(up.is_appx_framework, Some(true));

        let fam = p.family_metadata.as_ref().unwrap();
        assert_eq!(fam.name.as_deref(), Some("App.Pkg"));
        assert_eq!(fam.legacy_mobile_product_id.as_deref(), Some("legacy-1"));

        let cat = p.category_information.as_ref().unwrap();
        assert_eq!(cat.category_type.as_deref(), Some("App"));
        assert_eq!(cat.display_order, Some(5));
        assert_eq!(cat.prohibits_subcategories, Some(true));

        let dep = p.deployment.as_ref().unwrap();
        assert_eq!(dep.action.as_deref(), Some("Install"));
        assert_eq!(dep.flight_id.as_deref(), Some("ABC"));
        assert_eq!(dep.priority.as_deref(), Some("0"));

        // Nested subtrees preserved verbatim.
        assert!(p
            .applicability_rules_xml
            .as_deref()
            .unwrap()
            .contains("IsInstalled"));
        assert!(p
            .installation_behavior_xml
            .as_deref()
            .unwrap()
            .contains("RebootRequired"));

        // Catch-all: an unmapped attribute is preserved, not dropped.
        assert_eq!(
            p.extra_attributes
                .get("AppxMetadata@FutureAttr")
                .map(String::as_str),
            Some("surprise"),
        );
    }

    #[tokio::test]
    async fn get_package_instances_exposes_decoded_sha_hex() {
        // <File> with a base64 SHA-1 Digest + a base64 SHA-256 AdditionalDigest
        // (real values from a captured SyncUpdates response).
        let xml = r#"<?xml version="1.0"?>
<root>
  <NewUpdates><UpdateInfo><ID>1</ID><Xml>
    <UpdateIdentity UpdateID="u" RevisionNumber="1"/>
    <ApplicabilityRules><Metadata><AppxPackageMetadata>
      <AppxMetadata PackageMoniker="App.Pkg_1.0_x64__h" PackageType="AppX" IsAppxBundle="false">{}</AppxMetadata>
    </AppxPackageMetadata></Metadata></ApplicabilityRules>
  </Xml></UpdateInfo></NewUpdates>
  <ExtendedUpdateInfo><Updates><Update><ID>1</ID><Xml>
    <Files>
      <File FileName="g.appxbundle" InstallerSpecificIdentifier="App.Pkg_1.0_x64__h" Digest="SvGxBAsz2gqDUc3zvlYYqJCElQ0=" DigestAlgorithm="SHA1" Size="100">
        <AdditionalDigest Algorithm="SHA256">Cdmft9WJDkWAcAH7YZGK70c++XfGi0qLtrjswxq0hF8=</AdditionalDigest>
      </File>
    </Files>
  </Xml></Update></Updates></ExtendedUpdateInfo>
</root>"#;
        let pkgs = FE3Handler::get_package_instances(xml).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        let p = &pkgs[0];
        // Decoded, ready-to-use lowercase hex.
        assert_eq!(
            p.sha1.as_deref(),
            Some("4af1b1040b33da0a8351cdf3be5618a89084950d"),
        );
        assert_eq!(
            p.sha256.as_deref(),
            Some("09d99fb7d5890e45807001fb61918aef473ef977c68b4a8bb6b8ecc31ab4845f"),
        );
        assert_eq!(p.sha1.as_deref().map(str::len), Some(40));
        assert_eq!(p.sha256.as_deref().map(str::len), Some(64));
        // Raw base64 fields are preserved unchanged.
        assert_eq!(p.digest.as_deref(), Some("SvGxBAsz2gqDUc3zvlYYqJCElQ0="));
        assert_eq!(p.additional_digests.len(), 1);
    }

    #[tokio::test]
    async fn get_package_instances_no_relationships_yields_empty_prereqs() {
        // Same shape but no <Relationships> block at all.
        let xml = r#"<?xml version="1.0"?>
<root><UpdateInfo><ID>1</ID><Xml>
  <UpdateIdentity UpdateID="x" RevisionNumber="1"/>
  <ApplicabilityRules><Metadata><AppxPackageMetadata>
    <AppxMetadata PackageMoniker="App.Pkg_1.0_x64__hash" PackageType="AppX" IsAppxBundle="false">{}</AppxMetadata>
  </AppxPackageMetadata></Metadata></ApplicabilityRules>
</Xml></UpdateInfo></root>"#;
        let instances = FE3Handler::get_package_instances(xml).await.unwrap();
        assert_eq!(instances.len(), 1);
        assert!(instances[0].prerequisites.is_empty());
    }
}
