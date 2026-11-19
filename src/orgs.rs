//! The practices `modmed` reaches: ModMed's public endpoint directories
//! (one for EMA and its other EHRs, one for gGastro), downloaded and kept as
//! an index under `${XDG_CACHE_HOME:-~/.cache}/modmed`, and refreshed when
//! it is a week old. Each directory is a FHIR searchset of `Endpoint`
//! resources with their managing `Organization` included, 100 to a page.
//!
//! An organization's id is its FHIR host without ModMed's common suffix
//! (`example.mmi` for `example.mmi.prod.fhir.ema-api.com`): the practice's
//! subdomain and its zone, which together are unique, where the subdomain
//! alone is not (a few practices are listed in two zones).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pk_cli_core::transport::{require_secure_url, HttpRequest, Transport};
use pk_cli_core::{block_on, CliError};
use serde::{Deserialize, Serialize};

use crate::modmed::{self, TEST_ORG_ID, VENDOR};

/// ModMed's directories, as its endpoint page links them.
pub const DIRECTORIES: &[(&str, &str)] = &[
    (
        "ema",
        "https://public-api.mmi.prod.fhir.ema-api.com/fhir/r4/Endpoint?_include=Endpoint:managingOrganization&connection-type=hl7-fhir-rest",
    ),
    (
        "gastro",
        "https://public-api.gastro.prod.fhir.ema-api.com/fhir/r4/Endpoint?_include=Endpoint:managingOrganization&connection-type=hl7-fhir-rest",
    ),
];

/// The index's own format.
pub const INDEX_SCHEMA: &str = "modmed-orgs/v1";

/// How old the index may get before a command downloads it again.
pub const REFRESH_EVERY_SECS: i64 = 7 * 24 * 3600;

/// Pages one directory download follows at most (ModMed's EMA directory is
/// about 90 today).
pub const PAGE_CAP: usize = 400;

/// The suffix an organization id leaves off its FHIR host.
const HOST_SUFFIX: &str = ".prod.fhir.ema-api.com";

/// The NPI identifier system.
const NPI: &str = "http://hl7.org/fhir/sid/us-npi";

/// ModMed's identifier system for a practice's subdomain.
const SUBDOMAIN: &str = "http://fhir.ema-api.com/ns/subdomain";

/// One practice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Org {
    /// `example.mmi`: the FHIR host without `.prod.fhir.ema-api.com`.
    pub id: String,
    pub name: String,
    /// The FHIR base, normalized (`pk_med_fhir::normalize_base`).
    pub fhir_base: String,
    /// The base as the directory spells it, which is SMART's `aud`.
    pub address: String,
    /// Which directory listed it: `ema` or `gastro`.
    pub directory: String,
    /// The practice's subdomain as ModMed's identifier gives it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdomain: Option<String>,
    /// `City, ST` of its first address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// The two-letter state of that address, upper case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub npi: Vec<String>,
}

impl Org {
    /// The test server on this machine at `base`.
    pub fn test(base: &str) -> Self {
        let base = pk_med_fhir::normalize_base(base);
        Org {
            id: TEST_ORG_ID.into(),
            name: "Test server on this machine".into(),
            fhir_base: base.clone(),
            address: base,
            directory: "local".into(),
            subdomain: None,
            location: Some("this machine".into()),
            state: None,
            npi: Vec::new(),
        }
    }

    pub fn is_test(&self) -> bool {
        self.id == TEST_ORG_ID
    }

    /// SMART's `aud`: the directory's spelling without a trailing slash.
    pub fn aud(&self) -> &str {
        self.address.trim_end_matches('/')
    }

    /// As `org-list/v1` and a stream header carry it.
    pub fn organization(&self) -> pk_med_records::Organization {
        let mut aliases = Vec::new();
        if let Some(s) = &self.subdomain {
            aliases.push(s.clone());
        }
        aliases.extend(self.npi.iter().map(|n| format!("NPI {n}")));
        pk_med_records::Organization {
            id: self.id.clone(),
            name: self.name.clone(),
            fhir_base: self.fhir_base.clone(),
            vendor: Some(VENDOR.to_string()),
            location: self.location.clone(),
            aliases,
        }
    }
}

/// The cached index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgIndex {
    pub schema: String,
    /// The directory URLs it was built from.
    pub sources: Vec<String>,
    /// Unix seconds.
    pub fetched_at: i64,
    pub orgs: Vec<Org>,
}

impl OrgIndex {
    pub fn is_stale(&self, now: i64) -> bool {
        now - self.fetched_at >= REFRESH_EVERY_SECS
    }
}

/// An organization id from a FHIR base: its host, lower case, without
/// `.prod.fhir.ema-api.com` (or without `.fhir.ema-api.com` for a host in
/// another environment). `None` for an address that is not ModMed's https
/// FHIR host.
pub fn org_id(address: &str) -> Option<String> {
    if !modmed::is_modmed_fhir(address) {
        return None;
    }
    let url = url::Url::parse(address).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let id = host
        .strip_suffix(HOST_SUFFIX)
        .or_else(|| host.strip_suffix(&format!(".{}", modmed::FHIR_DOMAIN)))?;
    (!id.is_empty()).then(|| id.to_string())
}

// A directory page, read only as far as the index needs.
#[derive(Deserialize)]
struct Bundle {
    #[serde(default)]
    entry: Vec<Entry>,
    #[serde(default)]
    link: Vec<Link>,
}

#[derive(Deserialize)]
struct Link {
    #[serde(default)]
    relation: String,
    #[serde(default)]
    url: String,
}

#[derive(Deserialize)]
struct Entry {
    resource: Resource,
}

#[derive(Deserialize)]
#[serde(tag = "resourceType")]
enum Resource {
    Organization(Organization),
    Endpoint(Endpoint),
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct Organization {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    identifier: Vec<Identifier>,
    #[serde(default)]
    address: Vec<Address>,
}

#[derive(Deserialize)]
struct Identifier {
    #[serde(default)]
    system: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<Kind>,
    #[serde(default)]
    value: String,
}

#[derive(Deserialize)]
struct Kind {
    #[serde(default)]
    coding: Vec<Coding>,
}

#[derive(Deserialize)]
struct Coding {
    #[serde(default)]
    system: Option<String>,
}

#[derive(Deserialize)]
struct Address {
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

#[derive(Deserialize)]
struct Endpoint {
    #[serde(default)]
    status: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    address: String,
    #[serde(rename = "managingOrganization", default)]
    managing_organization: Option<Reference>,
}

#[derive(Deserialize)]
struct Reference {
    #[serde(default)]
    reference: String,
}

/// One page: its practices, and the `next` page's URL.
pub fn parse_page(text: &str, directory: &str) -> Result<(Vec<Org>, Option<String>), CliError> {
    let bundle: Bundle = serde_json::from_str(text).map_err(|e| {
        CliError::Upstream(format!(
            "ModMed's directory did not read as a FHIR Bundle: {}",
            pk_med_fhir::describe_json_error(&e)
        ))
    })?;
    let mut organizations: BTreeMap<String, &Organization> = BTreeMap::new();
    for e in &bundle.entry {
        if let Resource::Organization(o) = &e.resource {
            organizations.insert(format!("Organization/{}", o.id), o);
        }
    }
    let mut orgs = Vec::new();
    for e in &bundle.entry {
        let Resource::Endpoint(ep) = &e.resource else {
            continue;
        };
        if !ep.status.eq_ignore_ascii_case("active") {
            continue;
        }
        let Some(id) = org_id(&ep.address) else {
            continue;
        };
        let org = ep
            .managing_organization
            .as_ref()
            .and_then(|r| organizations.get(&r.reference));
        let name = org
            .map(|o| o.name.trim())
            .filter(|n| !n.is_empty())
            .unwrap_or(ep.name.trim())
            .to_string();
        let location = org.and_then(|o| {
            o.address.iter().find_map(|a| {
                let city = a.city.as_deref().map(str::trim).filter(|c| !c.is_empty());
                let state = a.state.as_deref().map(str::trim).filter(|s| !s.is_empty());
                match (city, state) {
                    (Some(c), Some(s)) => Some(format!("{c}, {s}")),
                    (Some(c), None) => Some(c.to_string()),
                    (None, Some(s)) => Some(s.to_string()),
                    (None, None) => None,
                }
            })
        });
        let state = org.and_then(|o| {
            o.address.iter().find_map(|a| {
                a.state
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| s.len() == 2 && s.chars().all(|c| c.is_ascii_alphabetic()))
                    .map(str::to_ascii_uppercase)
            })
        });
        let identifiers = org.map(|o| o.identifier.as_slice()).unwrap_or_default();
        let subdomain = identifiers
            .iter()
            .find(|i| {
                i.kind.as_ref().is_some_and(|k| {
                    k.coding
                        .iter()
                        .any(|c| c.system.as_deref() == Some(SUBDOMAIN))
                })
            })
            .map(|i| i.value.trim().to_string())
            .filter(|v| !v.is_empty());
        let npi = identifiers
            .iter()
            .filter(|i| i.system.as_deref() == Some(NPI))
            .map(|i| i.value.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect();
        orgs.push(Org {
            id,
            name,
            fhir_base: pk_med_fhir::normalize_base(&ep.address),
            address: ep.address.trim().to_string(),
            directory: directory.to_string(),
            subdomain,
            location,
            state,
            npi,
        });
    }
    let next = bundle
        .link
        .into_iter()
        .find(|l| l.relation == "next" && !l.url.trim().is_empty())
        .map(|l| l.url);
    Ok((orgs, next))
}

/// Merge what the pages listed: one per id, sorted by name then id. ModMed
/// lists some practices twice at one address, once without its
/// organization; the entry with a location and identifiers wins.
pub fn merge(found: Vec<Org>) -> Vec<Org> {
    let mut by_id: BTreeMap<String, Org> = BTreeMap::new();
    for org in found {
        let richer = |o: &Org| o.location.is_some() as u8 + (!o.npi.is_empty()) as u8;
        match by_id.get(&org.id) {
            Some(kept) if richer(kept) >= richer(&org) => {}
            _ => {
                by_id.insert(org.id.clone(), org);
            }
        }
    }
    let mut orgs: Vec<Org> = by_id.into_values().collect();
    orgs.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    orgs
}

/// `${XDG_CACHE_HOME:-~/.cache}/modmed`.
pub fn cache_dir() -> Result<PathBuf, CliError> {
    if let Some(x) = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(x).join("modmed"));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .ok_or_else(|| CliError::Other("cannot locate the home directory for the cache".into()))?;
    Ok(PathBuf::from(home).join(".cache").join("modmed"))
}

pub fn index_path(cache: &Path) -> PathBuf {
    cache.join("orgs.json")
}

pub fn load_index(cache: &Path) -> Result<Option<OrgIndex>, CliError> {
    let path = index_path(cache);
    match std::fs::read_to_string(&path) {
        Ok(s) => match serde_json::from_str::<OrgIndex>(&s) {
            Ok(i) if i.schema == INDEX_SCHEMA => Ok(Some(i)),
            // An index from another version is rebuilt, not an error.
            _ => Ok(None),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(CliError::Other(format!("reading {}: {e}", path.display()))),
    }
}

/// Whether a directory URL may be read: ModMed's https hosts, or a test
/// server on this machine.
pub fn allowed_directory(url: &str) -> bool {
    modmed::is_modmed_fhir(url) || modmed::is_loopback(url)
}

/// Every page of one directory, following `next` on the same origin.
fn download_one<T: Transport>(
    t: &T,
    directory: &str,
    url: &str,
    note: &dyn Fn(String),
) -> Result<Vec<Org>, CliError> {
    require_secure_url(url)?;
    if !allowed_directory(url) {
        return Err(CliError::Usage(
            "a directory is ModMed's (https under fhir.ema-api.com) or a test server on this machine"
                .into(),
        ));
    }
    let origin = |u: &str| url::Url::parse(u).map(|u| u.origin().ascii_serialization());
    let home = origin(url).map_err(|e| CliError::Usage(format!("not a URL: {e}")))?;
    let mut orgs = Vec::new();
    let mut next = Some(url.to_string());
    let mut pages = 0;
    while let Some(page) = next.take() {
        if pages == PAGE_CAP {
            return Err(CliError::Upstream(format!(
                "ModMed's {directory} directory ran past {PAGE_CAP} pages; nothing was kept"
            )));
        }
        let resp = block_on(
            t.send(
                HttpRequest::get(&page)
                    .header("Accept", "application/fhir+json, application/json")
                    .user_agent(crate::commands::BIN, env!("CARGO_PKG_VERSION")),
            ),
        )?;
        if !resp.is_success() {
            return Err(CliError::Upstream(format!(
                "downloading ModMed's {directory} directory failed (HTTP {} on page {})",
                resp.status,
                pages + 1
            )));
        }
        let (found, link) = parse_page(&resp.body, directory)?;
        orgs.extend(found);
        pages += 1;
        if pages % 20 == 0 {
            note(format!(
                "ModMed's {directory} directory: {pages} pages, {} practices so far",
                orgs.len()
            ));
        }
        if let Some(link) = link {
            if origin(&link).ok().as_deref() != Some(home.as_str()) {
                return Err(CliError::Upstream(format!(
                    "ModMed's {directory} directory linked its next page to another host; nothing was kept"
                )));
            }
            next = Some(link);
        }
    }
    Ok(orgs)
}

/// Download every directory and replace the index. `sources` are
/// `(name, url)` pairs: [`DIRECTORIES`], or a test server's.
pub fn download<T: Transport>(
    t: &T,
    cache: &Path,
    sources: &[(String, String)],
    now: i64,
    note: &dyn Fn(String),
) -> Result<OrgIndex, CliError> {
    let mut found = Vec::new();
    for (name, url) in sources {
        found.extend(download_one(t, name, url, note)?);
    }
    let index = OrgIndex {
        schema: INDEX_SCHEMA.into(),
        sources: sources.iter().map(|(_, u)| u.clone()).collect(),
        fetched_at: now,
        orgs: merge(found),
    };
    if index.orgs.is_empty() {
        return Err(CliError::Upstream(
            "ModMed's directories listed no practice; the index was kept as it was".into(),
        ));
    }
    let body = serde_json::to_vec(&index)
        .map_err(|e| CliError::Other(format!("serializing the directory index: {e}")))?;
    crate::fsutil::write_private(&index_path(cache), &body)?;
    Ok(index)
}

/// What [`ensure`] did.
pub enum Freshness {
    /// The cached index was young enough.
    Cached,
    /// Downloaded now.
    Refreshed,
    /// The download failed; the stale index is in use (the failure, for a
    /// warning).
    Stale(CliError),
}

/// The index, downloading the directories first when there is none, it is
/// a week old, it came from other sources, or `force`. A failed download
/// with an older index still answers from that index.
pub fn ensure<T: Transport>(
    t: &T,
    cache: &Path,
    sources: &[(String, String)],
    now: i64,
    force: bool,
    note: &dyn Fn(String),
) -> Result<(OrgIndex, Freshness), CliError> {
    let cached = load_index(cache)?;
    let urls: Vec<String> = sources.iter().map(|(_, u)| u.clone()).collect();
    match cached {
        Some(i) if !force && !i.is_stale(now) && i.sources == urls => Ok((i, Freshness::Cached)),
        cached => match download(t, cache, sources, now, note) {
            Ok(i) => Ok((i, Freshness::Refreshed)),
            Err(e) => match cached {
                Some(old) if e.exit_code() == 5 => Ok((old, Freshness::Stale(e))),
                _ => Err(e),
            },
        },
    }
}

/// Resolve `--org`: an id (`example.mmi`), a subdomain, an NPI, or a name
/// (exact, then any case, then a unique part), per
/// `pk_cli_core::resolve::pick`.
pub fn resolve<'a>(orgs: &'a [Org], query: &str) -> Result<&'a Org, CliError> {
    pk_cli_core::resolve::pick(
        orgs,
        query,
        |o| {
            let mut ids = vec![o.id.clone()];
            if let Some(s) = &o.subdomain {
                ids.push(s.to_lowercase());
                ids.push(s.clone());
            }
            ids.extend(o.npi.iter().cloned());
            ids
        },
        |o| &o.name,
        "organization",
    )
    .map_err(|e| {
        e.with_remedy("find it with `modmed orgs search <text>` and pass its id (`--org <ID>`)")
    })
}

/// Practices whose name, id, subdomain, location or NPI contains every word
/// of `text` (any case).
pub fn search<'a>(orgs: &'a [Org], text: &str) -> Vec<&'a Org> {
    let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    orgs.iter()
        .filter(|o| {
            let mut hay = vec![o.name.clone(), o.id.clone()];
            hay.extend(o.subdomain.clone());
            hay.extend(o.location.clone());
            hay.extend(o.npi.iter().cloned());
            let hay = hay.join(" ").to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = include_str!("../tests/fixtures/directory/ema-page1.json");
    const LAST: &str = include_str!("../tests/fixtures/directory/ema-page2.json");

    #[test]
    fn an_id_is_the_host_without_modmeds_suffix() {
        assert_eq!(
            org_id("https://example.mmi.prod.fhir.ema-api.com/fhir/r4/").as_deref(),
            Some("example.mmi")
        );
        assert_eq!(
            org_id("https://EXAMPLE.EF.prod.fhir.ema-api.com/fhir/r4").as_deref(),
            Some("example.ef")
        );
        assert_eq!(
            org_id("https://x.stage.fhir.ema-api.com/fhir/r4").as_deref(),
            Some("x.stage")
        );
        for bad in [
            "http://example.mmi.prod.fhir.ema-api.com/fhir/r4",
            "https://example.example.com/fhir",
            "https://fhir.ema-api.com/fhir",
            "not a url",
        ] {
            assert_eq!(org_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_page_yields_its_active_practices_and_the_next_link() {
        let (orgs, next) = parse_page(PAGE, "ema").unwrap();
        let ids: Vec<&str> = orgs.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "exampledermatology.mmi",
                "sampleeye.ef",
                "sampleeye.ef",
                "democlinic.mmi"
            ]
        );
        let first = &orgs[0];
        assert_eq!(first.name, "Example Dermatology Associates");
        assert_eq!(first.location.as_deref(), Some("Springfield, ST"));
        assert_eq!(first.npi, ["1555000001"]);
        assert_eq!(first.subdomain.as_deref(), Some("exampledermatology"));
        assert_eq!(
            first.fhir_base,
            "https://exampledermatology.mmi.prod.fhir.ema-api.com/fhir/r4"
        );
        assert_eq!(
            first.aud(),
            "https://exampledermatology.mmi.prod.fhir.ema-api.com/fhir/r4"
        );
        // The second listing at one address has no organization: the
        // endpoint's own name, no location.
        assert_eq!(orgs[2].name, "Sample Eye Center");
        assert_eq!(orgs[2].location, None);
        assert!(next.unwrap().contains("_offset=2"));
        let (last, none) = parse_page(LAST, "ema").unwrap();
        assert_eq!(last.len(), 1);
        assert!(none.is_none());
    }

    #[test]
    fn merging_keeps_one_per_id_the_richer_listing() {
        let (orgs, _) = parse_page(PAGE, "ema").unwrap();
        let merged = merge(orgs);
        assert_eq!(merged.len(), 3);
        let eye = merged.iter().find(|o| o.id == "sampleeye.ef").unwrap();
        assert_eq!(eye.location.as_deref(), Some("Shelbyville, ST"));
        let names: Vec<&str> = merged.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Demo Clinic",
                "Example Dermatology Associates",
                "Sample Eye Center"
            ]
        );
    }

    #[test]
    fn resolve_and_search() {
        let (orgs, _) = parse_page(PAGE, "ema").unwrap();
        let orgs = merge(orgs);
        assert_eq!(
            resolve(&orgs, "exampledermatology.mmi").unwrap().id,
            "exampledermatology.mmi"
        );
        assert_eq!(
            resolve(&orgs, "exampledermatology").unwrap().id,
            "exampledermatology.mmi"
        );
        assert_eq!(
            resolve(&orgs, "1555000001").unwrap().id,
            "exampledermatology.mmi"
        );
        assert_eq!(resolve(&orgs, "sample eye").unwrap().id, "sampleeye.ef");
        let err = resolve(&orgs, "nowhere").unwrap_err();
        assert_eq!(err.exit_code(), 4);
        assert!(err.remedy().unwrap().contains("orgs search"));
        assert_eq!(search(&orgs, "springfield").len(), 1);
        assert_eq!(search(&orgs, "DERMATOLOGY springfield").len(), 1);
        assert_eq!(search(&orgs, "clinic").len(), 1);
        assert!(search(&orgs, "  ").is_empty());
    }

    #[test]
    fn an_org_is_a_modmed_organization() {
        let (orgs, _) = parse_page(PAGE, "ema").unwrap();
        let o = orgs[0].organization();
        assert_eq!(o.vendor.as_deref(), Some("modmed"));
        assert_eq!(o.aliases, ["exampledermatology", "NPI 1555000001"]);
        let t = Org::test("http://127.0.0.1:9/fhir/");
        assert!(t.is_test());
        assert_eq!(t.fhir_base, "http://127.0.0.1:9/fhir");
    }

    #[test]
    fn a_page_that_is_not_a_bundle_is_an_upstream_error() {
        assert_eq!(parse_page("<html>", "ema").unwrap_err().exit_code(), 5);
    }

    #[test]
    fn directories_are_modmeds_or_this_machines() {
        for (_, url) in DIRECTORIES {
            assert!(allowed_directory(url), "{url}");
        }
        assert!(allowed_directory("http://127.0.0.1:9/fhir/Endpoint"));
        assert!(!allowed_directory("https://example.com/Endpoint"));
    }
}
