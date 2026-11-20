//! docs/registration.md and `modmed registration` say the same thing: every
//! app field's label and value, the scopes the boxes add up to, and the 27
//! types in the order the sign-in asks for them.

use std::collections::BTreeSet;

use modmed::modmed::{CONTEXT_SCOPES, PATIENT_TYPES};
use modmed::registration::{registration, REDIRECT_PAGE};

fn doc() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/registration.md")).unwrap()
}

#[test]
fn the_doc_gives_every_app_field_the_command_s_value() {
    let doc = doc();
    let reg = registration(REDIRECT_PAGE, false);
    for f in reg.fields.iter().chain(&reg.urls) {
        let row = doc
            .lines()
            .find(|l| l.starts_with(&format!("| {} |", f.label)))
            .unwrap_or_else(|| panic!("no row for {} in the app table", f.label));
        assert!(row.contains(&f.value), "{}: {row}", f.label);
    }
    assert!(doc.contains(&reg.portal));
    assert!(doc.contains(&format!("`{}`", reg.store)));
}

#[test]
fn the_doc_s_boxes_are_the_command_s_scopes() {
    let doc = doc();
    let rows: Vec<&str> = doc
        .lines()
        .skip_while(|l| !l.starts_with("| Group on the form |"))
        .skip(2)
        .take_while(|l| l.starts_with('|'))
        .collect();
    assert_eq!(rows.len(), 4);
    let mut checked = BTreeSet::new();
    for row in rows {
        let check = row.split('|').nth(2).unwrap();
        if check.contains("all 27 `patient/<Type>.rs`") {
            checked.extend(PATIENT_TYPES.iter().map(|t| format!("patient/{t}.rs")));
        } else if check.trim() != "none" {
            checked.extend(check.split('`').skip(1).step_by(2).map(str::to_string));
        }
    }
    let reg: BTreeSet<String> = registration(REDIRECT_PAGE, false)
        .scopes
        .into_iter()
        .collect();
    assert_eq!(checked, reg);
    assert!(CONTEXT_SCOPES.iter().all(|s| reg.contains(*s)));
}

#[test]
fn the_doc_lists_the_27_types_in_the_sign_in_s_order() {
    let doc = doc();
    let start = doc.find("The 27 types are ").unwrap() + "The 27 types are ".len();
    let end = start + doc[start..].find(", as `modmed registration`").unwrap();
    let listed: Vec<String> = doc[start..end]
        .replace(" and ", ", ")
        .split(',')
        .map(|t| t.split_whitespace().collect::<String>())
        .collect();
    assert_eq!(listed, PATIENT_TYPES);
}
