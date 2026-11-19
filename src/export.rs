//! What `records export` pulls from a practice's Certified FHIR API, in
//! order:
//!
//! 1. the patient's `Patient` record;
//! 2. a search by `patient` of each type ModMed's CapabilityStatement serves
//!    for a patient ([`SEARCHES`]). Under `--since`, a type with a date
//!    parameter is bounded by it (`date`, `authoredon`, `authored`); the
//!    CapabilityStatement offers no `_lastUpdated`, so the rest are read
//!    whole. A search the server refuses as a bad request is asked again
//!    once per value of the parameter US Core makes it require
//!    ([`Search::split`]);
//! 3. right after a DocumentReference or DiagnosticReport, each attachment's
//!    `Binary` on this server. An attachment carried inline (`data`) is
//!    already in the resource;
//! 4. a read of each resource the searches name by reference
//!    ([`REFERENCED`]).
//!
//! A refused or failed search or read is a [`Skip`], and so is what a
//! search's answer says it left out; the export stops only when the sign-in,
//! the network or the output fails.

use std::collections::BTreeSet;
use std::io::Write;

use pk_cli_core::transport::Transport;
use pk_cli_core::{block_on, CliError, Reason};
use pk_med_fhir::{is_id, FhirClient, RawResource, ReferenceTarget};
use pk_med_records::stream::skip;
use pk_med_records::{RecordsWriter, Skip};
use serde_json::Value;

/// One search by `patient`.
#[derive(Debug, Clone, Copy)]
pub struct Search {
    pub resource_type: &'static str,
    /// The date parameter `--since` bounds it by; `None` reads it whole.
    pub date: Option<&'static str>,
    /// The parameter and values to search by one at a time when the server
    /// refuses the search by `patient` alone as a bad request: US Core
    /// makes some servers require a category, status or intent.
    pub split: Option<(&'static str, &'static [&'static str])>,
}

const fn by_patient(resource_type: &'static str) -> Search {
    Search {
        resource_type,
        date: None,
        split: None,
    }
}

const fn dated(resource_type: &'static str, date: &'static str) -> Search {
    Search {
        resource_type,
        date: Some(date),
        split: None,
    }
}

const fn split(s: Search, param: &'static str, values: &'static [&'static str]) -> Search {
    Search {
        split: Some((param, values)),
        ..s
    }
}

/// US Core's Observation categories.
pub const OBSERVATION_CATEGORIES: &[&str] = &[
    "laboratory",
    "vital-signs",
    "social-history",
    "survey",
    "exam",
    "imaging",
    "procedure",
    "therapy",
    "activity",
    "sdoh",
    "clinical-test",
    "symptom",
];

/// US Core's DiagnosticReport categories: laboratory, and the LOINC
/// note categories (cardiology, radiology, pathology).
pub const REPORT_CATEGORIES: &[&str] = &["LAB", "LP29708-2", "LP29684-5", "LP7839-6"];

/// The searches by `patient`, in pull order.
pub const SEARCHES: &[Search] = &[
    by_patient("AllergyIntolerance"),
    by_patient("Condition"),
    split(
        dated("MedicationRequest", "authoredon"),
        "intent",
        &[
            "proposal",
            "plan",
            "order",
            "original-order",
            "reflex-order",
            "filler-order",
            "instance-order",
            "option",
        ],
    ),
    by_patient("MedicationDispense"),
    split(
        dated("Observation", "date"),
        "category",
        OBSERVATION_CATEGORIES,
    ),
    split(
        dated("DiagnosticReport", "date"),
        "category",
        REPORT_CATEGORIES,
    ),
    dated("Procedure", "date"),
    dated("Immunization", "date"),
    dated("Encounter", "date"),
    split(
        dated("DocumentReference", "date"),
        "category",
        &["clinical-note"],
    ),
    split(by_patient("CarePlan"), "category", &["assess-plan"]),
    split(
        by_patient("CareTeam"),
        "status",
        &[
            "proposed",
            "active",
            "suspended",
            "inactive",
            "entered-in-error",
        ],
    ),
    by_patient("Goal"),
    by_patient("Device"),
    dated("ServiceRequest", "authored"),
    by_patient("Specimen"),
    by_patient("Coverage"),
    by_patient("QuestionnaireResponse"),
    by_patient("RelatedPerson"),
];

/// The types a reference is read as, sorted.
pub const REFERENCED: &[&str] = &[
    "Location",
    "Medication",
    "Organization",
    "Practitioner",
    "PractitionerRole",
];

/// Types whose attachments are fetched as their `Binary`, right after them.
pub fn has_files(resource_type: &str) -> bool {
    matches!(resource_type, "DocumentReference" | "DiagnosticReport")
}

/// The types a `--since` export reads whole rather than bounded: the
/// patient's record, the searches with no date parameter, and what the
/// rest reference.
pub fn since_unbounded() -> Vec<String> {
    let mut types: Vec<&str> = vec!["Patient"];
    types.extend(
        SEARCHES
            .iter()
            .filter(|s| s.date.is_none())
            .map(|s| s.resource_type),
    );
    types.extend(REFERENCED.iter().copied());
    types.sort();
    types.dedup();
    types.into_iter().map(str::to_string).collect()
}

/// Whether a failure ends the export rather than skipping one item: the
/// sign-in refused or gone, or the server unreachable.
fn stops(err: &CliError) -> bool {
    matches!(
        err.reason(),
        Some(Reason::CredentialRefused) | Some(Reason::NoCredential)
    ) || err.reason() == Some(pk_med_fhir::UNREACHABLE)
        || (err.exit_code() == 3 && err.reason() != Some(Reason::Forbidden))
}

fn skip_code(err: &CliError) -> &'static str {
    match err.exit_code() {
        3 => skip::FORBIDDEN,
        4 => skip::NOT_FOUND,
        _ => skip::UPSTREAM,
    }
}

fn search_skip_code(err: &CliError) -> &'static str {
    if err.exit_code() == 4 {
        skip::UNSUPPORTED
    } else {
        skip_code(err)
    }
}

/// `Type?a=b&c=d` without the patient's id, for a skip's `search`.
fn label(resource_type: &str, params: &[(&str, String)]) -> String {
    let rest: Vec<String> = params
        .iter()
        .filter(|(k, _)| *k != "patient")
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    if rest.is_empty() {
        resource_type.to_string()
    } else {
        format!("{resource_type}?{}", rest.join("&"))
    }
}

/// What one export has seen so far.
#[derive(Default)]
struct Pull {
    wanted: Wanted,
    /// What a search's answer said it left out, by type: part of the type's
    /// data, not of one search, so one skip per type and line however many
    /// searches carried it (medical-common DESIGN §1.4).
    withheld: BTreeSet<(&'static str, String)>,
    /// Binary ids fetched or skipped, so each is tried once.
    binaries: BTreeSet<String>,
}

/// Pull everything for `patient` into `w`.
pub fn run<T: Transport, W: Write>(
    fhir: &FhirClient<T>,
    w: &mut RecordsWriter<W>,
    patient: &str,
    since: Option<&str>,
    note: &dyn Fn(String),
) -> Result<(), CliError> {
    let base = fhir.base().to_string();
    let mut pull = Pull::default();

    match block_on(fhir.read("Patient", patient)) {
        Ok(r) => {
            w.resource(&r)?;
            references(&r, &base, &mut pull.wanted);
        }
        Err(e) if stops(&e) => return Err(e),
        Err(e) => w.skip(Skip::resource("Patient", "", skip_code(&e), e.message())),
    }

    for search in SEARCHES {
        let mut params = vec![("patient", patient.to_string())];
        if let (Some(param), Some(day)) = (search.date, since) {
            params.push((param, format!("ge{day}")));
        }
        match one_search(fhir, w, &mut pull, search.resource_type, &params, note)? {
            None => {}
            Some(e) => match search.split {
                Some((param, values)) if e.reason() == Some(Reason::BadRequest) => {
                    note(format!(
                        "{}: refused by patient alone; searching by {param}",
                        search.resource_type
                    ));
                    for value in values {
                        let mut by = params.clone();
                        by.push((param, value.to_string()));
                        if let Some(e) =
                            one_search(fhir, w, &mut pull, search.resource_type, &by, note)?
                        {
                            w.skip(Skip::search(
                                label(search.resource_type, &by),
                                search_skip_code(&e),
                                e.message(),
                            ));
                        }
                    }
                }
                _ => w.skip(Skip::search(
                    label(search.resource_type, &params),
                    search_skip_code(&e),
                    e.message(),
                )),
            },
        }
    }

    for (resource_type, line) in &pull.withheld {
        w.skip(Skip::resource(
            *resource_type,
            "",
            skip::FORBIDDEN,
            line.as_str(),
        ));
    }
    for (resource_type, reason) in &pull.wanted.refused {
        w.skip(Skip::resource(
            resource_type.as_str(),
            "",
            skip::INVALID_REFERENCE,
            reason.as_str(),
        ));
    }
    let mut read = 0;
    for (resource_type, id) in std::mem::take(&mut pull.wanted.reads) {
        if w.written(&resource_type, &id) {
            continue;
        }
        match block_on(fhir.read(&resource_type, &id)) {
            Ok(r) => {
                if w.resource(&r)? {
                    read += 1;
                }
            }
            Err(e) if stops(&e) => return Err(e),
            Err(e) => w.skip(Skip::resource(
                resource_type.as_str(),
                id.as_str(),
                skip_code(&e),
                e.message(),
            )),
        }
    }
    note(format!("referenced resources: {read}"));
    Ok(())
}

/// One search: its resources written, each file's Binary after the resource
/// naming it. `Ok(Some(e))` is a search that failed without ending the
/// export, for the caller to skip or split.
fn one_search<T: Transport, W: Write>(
    fhir: &FhirClient<T>,
    w: &mut RecordsWriter<W>,
    pull: &mut Pull,
    resource_type: &'static str,
    params: &[(&str, String)],
    note: &dyn Fn(String),
) -> Result<Option<CliError>, CliError> {
    let base = fhir.base().to_string();
    let found = match block_on(fhir.search(resource_type, params)) {
        Ok(found) => found,
        Err(e) if stops(&e) => return Err(e),
        Err(e) => return Ok(Some(e)),
    };
    note(format!(
        "{}: {} over {} page(s)",
        label(resource_type, params),
        found.resources.len(),
        found.pages
    ));
    for r in found.resources.iter().chain(&found.included) {
        if w.resource(r)? && has_files(&r.resource_type) {
            if let Ok(v) = r.value() {
                binaries(fhir, w, &v, &base, &mut pull.binaries)?;
            }
        }
        references(r, &base, &mut pull.wanted);
    }
    for line in found.withheld() {
        pull.withheld.insert((resource_type, line));
    }
    if found.truncated {
        w.skip(Skip::search(
            label(resource_type, params),
            skip::PAGE_CAP,
            format!("stopped after {} pages", found.pages),
        ));
    }
    Ok(None)
}

/// What an attachment holds.
#[derive(Debug, PartialEq, Eq)]
pub enum Attachment {
    /// A Binary on this server, by id.
    Binary(String),
    /// Its bytes inline, in `data`: already in the resource.
    Inline,
    /// A URL that is not a Binary on this server, which `modmed` does not
    /// follow (`unsupported`).
    Elsewhere,
    /// A Binary reference whose id FHIR does not allow; the reason names its
    /// length, never the id.
    BadId(String),
    /// Neither a URL nor data.
    Empty,
}

/// The attachments of a DocumentReference (`content[].attachment`) or a
/// DiagnosticReport (`presentedForm[]`), in order.
pub fn attachments(v: &Value) -> Vec<&Value> {
    let mut out: Vec<&Value> = v["content"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| &c["attachment"])
        .filter(|a| a.is_object())
        .collect();
    out.extend(v["presentedForm"].as_array().into_iter().flatten());
    out
}

/// Where an attachment's bytes are, against the FHIR base.
pub fn attachment(a: &Value, base: &str) -> Attachment {
    match a["url"].as_str().map(str::trim).filter(|u| !u.is_empty()) {
        Some(url) => match ReferenceTarget::read(url) {
            Ok(Some(t)) if t.resource_type == "Binary" && t.is_under(base) => {
                Attachment::Binary(t.id.to_string())
            }
            Err(r) if r.resource_type == "Binary" && r.is_under(base) => {
                if is_id(r.id) {
                    Attachment::Binary(r.id.to_string())
                } else {
                    Attachment::BadId(r.to_string())
                }
            }
            _ if a["data"].is_string() => Attachment::Inline,
            _ => Attachment::Elsewhere,
        },
        None if a["data"].is_string() => Attachment::Inline,
        None => Attachment::Empty,
    }
}

/// Each attachment's Binary, written right after the resource naming it.
/// A Binary is fetched once per export: one that failed stays a skip rather
/// than landing later after some other resource.
fn binaries<T: Transport, W: Write>(
    fhir: &FhirClient<T>,
    w: &mut RecordsWriter<W>,
    v: &Value,
    base: &str,
    tried: &mut BTreeSet<String>,
) -> Result<(), CliError> {
    let owner = format!(
        "{}/{}",
        v["resourceType"].as_str().unwrap_or_default(),
        v["id"].as_str().unwrap_or_default()
    );
    for a in attachments(v) {
        let id = match attachment(a, base) {
            Attachment::Binary(id) => id,
            Attachment::Inline => continue,
            Attachment::Elsewhere => {
                w.skip(Skip::resource(
                    "Binary",
                    "",
                    skip::UNSUPPORTED,
                    format!("an attachment of {owner} is not a Binary on this server"),
                ));
                continue;
            }
            Attachment::BadId(reason) => {
                w.skip(Skip::resource(
                    "Binary",
                    "",
                    skip::INVALID_REFERENCE,
                    reason,
                ));
                continue;
            }
            Attachment::Empty => {
                w.skip(Skip::resource(
                    "Binary",
                    "",
                    skip::NO_FILE,
                    format!("an attachment of {owner} has neither a URL nor data"),
                ));
                continue;
            }
        };
        if !tried.insert(id.clone()) || w.written("Binary", &id) {
            continue;
        }
        match block_on(fhir.binary(&format!("Binary/{id}"))) {
            Ok(b) => {
                // A Binary without an id is written with the one its
                // attachment names. A Binary holds no decimal, so the round
                // trip through a Value changes nothing else.
                let b = if b.id.is_empty() {
                    let mut value = b.value()?;
                    value["id"] = id.clone().into();
                    RawResource::from_value(&value)?
                } else {
                    b
                };
                w.resource(&b)?;
            }
            Err(e) if stops(&e) => return Err(e),
            Err(e) => w.skip(Skip::resource(
                "Binary",
                id.as_str(),
                skip_code(&e),
                e.message(),
            )),
        }
    }
    Ok(())
}

/// What the pulled resources name on this server, as [`references`] finds it.
#[derive(Debug, Default)]
pub struct Wanted {
    /// `(type, id)` to read.
    pub reads: BTreeSet<(String, String)>,
    /// `(type, reason)` for a reference to a [`REFERENCED`] type on this
    /// server whose id FHIR does not allow: a skip, since no read can ask for
    /// it. The reason names the type and the id's length, never the id.
    pub refused: BTreeSet<(String, String)>,
}

/// Every `reference` in `r` to a [`REFERENCED`] type on this server, relative
/// or absolute under `base`, into `out`. Contained (`#…`), `urn:`,
/// conditional and other servers' references are left alone.
pub fn references(r: &RawResource, base: &str, out: &mut Wanted) {
    let Ok(value) = r.value() else { return };
    let mut stack = vec![&value];
    while let Some(v) = stack.pop() {
        match v {
            Value::Object(map) => {
                for (key, child) in map {
                    if key == "reference" {
                        if let Some(reference) = child.as_str() {
                            reference_into(reference, base, out);
                        }
                    } else {
                        stack.push(child);
                    }
                }
            }
            Value::Array(items) => stack.extend(items),
            _ => {}
        }
    }
}

fn reference_into(reference: &str, base: &str, out: &mut Wanted) {
    match ReferenceTarget::read(reference) {
        Ok(Some(t)) if t.is_under(base) && REFERENCED.contains(&t.resource_type) => {
            out.reads
                .insert((t.resource_type.to_string(), t.id.to_string()));
        }
        Ok(_) => {}
        Err(refused) if refused.is_under(base) && REFERENCED.contains(&refused.resource_type) => {
            if is_id(refused.id) {
                // Only the `_history` version is refused; the current
                // version of every reference is read anyway.
                out.reads
                    .insert((refused.resource_type.to_string(), refused.id.to_string()));
            } else {
                out.refused
                    .insert((refused.resource_type.to_string(), refused.to_string()));
            }
        }
        Err(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BASE: &str = "https://example.mmi.prod.fhir.ema-api.com/fhir/r4";

    #[test]
    fn every_search_is_a_type_the_scope_asks_for() {
        for s in SEARCHES {
            assert!(
                crate::modmed::PATIENT_TYPES.contains(&s.resource_type),
                "{}",
                s.resource_type
            );
        }
        for t in REFERENCED {
            assert!(crate::modmed::PATIENT_TYPES.contains(t), "{t}");
        }
        let mut names: Vec<&str> = SEARCHES.iter().map(|s| s.resource_type).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), SEARCHES.len());
    }

    #[test]
    fn the_unbounded_types_are_the_undated_searches_and_the_reads() {
        let types = since_unbounded();
        assert!(types.contains(&"Patient".to_string()));
        assert!(types.contains(&"Condition".to_string()));
        assert!(types.contains(&"Medication".to_string()));
        assert!(!types.contains(&"Observation".to_string()));
        assert!(!types.contains(&"Binary".to_string()));
        let mut sorted = types.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted, types);
    }

    #[test]
    fn a_label_leaves_the_patient_out() {
        let params = vec![
            ("patient", "pat-0001".to_string()),
            ("date", "ge2024-01-01".to_string()),
            ("category", "laboratory".to_string()),
        ];
        assert_eq!(
            label("Observation", &params),
            "Observation?date=ge2024-01-01&category=laboratory"
        );
        assert_eq!(
            label("Condition", &[("patient", "pat-0001".into())]),
            "Condition"
        );
    }

    #[test]
    fn attachments_are_binaries_here_inline_or_elsewhere() {
        let doc = json!({
            "resourceType": "DocumentReference", "id": "doc-1",
            "content": [
                {"attachment": {"url": "Binary/bin-1", "contentType": "application/pdf"}},
                {"attachment": {"url": format!("{BASE}/Binary/bin-2")}},
                {"attachment": {"data": "JVBERi0=", "contentType": "application/pdf"}},
                {"attachment": {"url": "https://elsewhere.example/doc.pdf"}},
                {"attachment": {"url": "Binary/not an id"}},
                {"attachment": {"contentType": "text/plain"}}
            ]
        });
        let got: Vec<Attachment> = attachments(&doc)
            .into_iter()
            .map(|a| attachment(a, BASE))
            .collect();
        assert_eq!(got[0], Attachment::Binary("bin-1".into()));
        assert_eq!(got[1], Attachment::Binary("bin-2".into()));
        assert_eq!(got[2], Attachment::Inline);
        assert_eq!(got[3], Attachment::Elsewhere);
        match &got[4] {
            Attachment::BadId(reason) => assert!(!reason.contains("not an id"), "{reason}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(got[5], Attachment::Empty);
        let report =
            json!({"resourceType": "DiagnosticReport", "presentedForm": [{"url": "Binary/bin-3"}]});
        assert_eq!(
            attachment(attachments(&report)[0], BASE),
            Attachment::Binary("bin-3".into())
        );
        assert!(has_files("DocumentReference") && !has_files("Observation"));
    }

    #[test]
    fn references_are_relative_or_on_this_base() {
        let r = RawResource::parse(&format!(
            r##"{{"resourceType":"MedicationRequest","id":"mr-0001",
            "subject":{{"reference":"Patient/pat-0001"}},
            "medicationReference":{{"reference":"Medication/med-0001"}},
            "requester":{{"reference":"{BASE}/Practitioner/prac-0001"}},
            "performer":{{"reference":"https://elsewhere.example.org/r4/Location/loc-9"}},
            "recorder":{{"reference":"PractitionerRole/role-0001/_history/2"}},
            "x":[{{"reference":"Organization/org-0001"}},{{"reference":"#contained"}},
                 {{"reference":"Location/bad id"}}]}}"##
        ))
        .unwrap();
        let mut out = Wanted::default();
        references(&r, BASE, &mut out);
        let got: Vec<String> = out.reads.iter().map(|(t, i)| format!("{t}/{i}")).collect();
        assert_eq!(
            got,
            [
                "Medication/med-0001",
                "Organization/org-0001",
                "Practitioner/prac-0001",
                "PractitionerRole/role-0001",
            ]
        );
        let refused: Vec<&str> = out.refused.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(refused, ["Location"]);
        for (_, reason) in &out.refused {
            assert!(!reason.contains("bad id"), "{reason}");
        }
    }

    #[test]
    fn only_a_refused_sign_in_or_an_unreachable_server_stops() {
        let refused = CliError::Auth("x".into()).because(Reason::CredentialRefused);
        let forbidden = CliError::Auth("x".into()).because(Reason::Forbidden);
        let unreachable = CliError::Upstream("x".into()).because(pk_med_fhir::UNREACHABLE);
        assert!(stops(&refused));
        assert!(stops(&unreachable));
        assert!(!stops(&forbidden));
        assert!(!stops(&CliError::NotFound("x".into())));
        assert!(!stops(&CliError::Upstream("x".into())));
        assert_eq!(skip_code(&forbidden), skip::FORBIDDEN);
        assert_eq!(
            search_skip_code(&CliError::NotFound("x".into())),
            skip::UNSUPPORTED
        );
    }
}
