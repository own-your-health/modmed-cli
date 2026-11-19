//! The directory fixtures keep the dummy shapes `tests/fixtures/README.md`
//! states, so a real practice's values never reach them, and they parse as
//! ModMed's directory does.

use serde_json::Value;

const PAGES: &[&str] = &[
    include_str!("fixtures/directory/ema-page1.json"),
    include_str!("fixtures/directory/ema-page2.json"),
];

const NAMES: &[&str] = &["Example", "Sample", "Demo", "Closed", "Elsewhere"];

fn resources(page: &str) -> Vec<Value> {
    let v: Value = serde_json::from_str(page).unwrap();
    v["entry"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["resource"].clone())
        .collect()
}

#[test]
fn identity_bearing_fields_are_dummies() {
    for page in PAGES {
        for r in resources(page) {
            let name = r["name"].as_str().unwrap_or_default();
            assert!(
                NAMES.iter().any(|n| name.starts_with(n)),
                "a name outside the dummy set: {name}"
            );
            let id = r["id"].as_str().unwrap();
            assert!(
                id.starts_with("1.5550") || id.starts_with("1.efpod"),
                "{id}"
            );
            if r["resourceType"] == "Endpoint" {
                let address = r["address"].as_str().unwrap();
                assert!(
                    address.contains(".prod.fhir.ema-api.com/")
                        || address.contains("elsewhere.example"),
                    "{address}"
                );
            }
            for i in r["identifier"].as_array().into_iter().flatten() {
                if i["system"] == "http://hl7.org/fhir/sid/us-npi" {
                    assert!(i["value"].as_str().unwrap().starts_with("1555"));
                }
            }
            for t in r["telecom"].as_array().into_iter().flatten() {
                assert!(t["value"].as_str().unwrap().starts_with("555-01"));
            }
            for a in r["address"].as_array().into_iter().flatten() {
                for line in a["line"].as_array().into_iter().flatten() {
                    assert!(line.as_str().unwrap().ends_with("Example St"), "{line}");
                }
                if let Some(code) = a["postalCode"].as_str() {
                    assert_eq!(code, "00000");
                }
                assert_eq!(a["state"], "ST");
            }
        }
    }
}

#[test]
fn the_pages_parse_as_modmeds_directory() {
    let (first, next) = modmed::orgs::parse_page(PAGES[0], "ema").unwrap();
    assert!(next.is_some());
    let (last, none) = modmed::orgs::parse_page(PAGES[1], "ema").unwrap();
    assert!(none.is_none());
    let all = modmed::orgs::merge(first.into_iter().chain(last).collect());
    assert_eq!(all.len(), 4);
    // The table view's columns are filled for a listed practice.
    for o in &all {
        assert!(!o.id.is_empty() && !o.name.is_empty() && !o.fhir_base.is_empty());
    }
}
