//! docs/registration.md and `modmed registration` say the same thing: every
//! app field's label and value, the scope groups and boxes, and the 27
//! types in the order the sign-in asks for them.

use modmed::modmed::PATIENT_TYPES;
use modmed::registration::{registration, REDIRECT_PAGE};

fn doc() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/registration.md")).unwrap()
}

#[test]
fn the_doc_gives_every_app_field_the_command_s_value() {
    let doc = doc();
    let r = registration(REDIRECT_PAGE, false);
    for f in &r.app {
        let row = doc
            .lines()
            .find(|l| l.starts_with(&format!("| {} |", f.label)))
            .unwrap_or_else(|| panic!("no row for {} in the app table", f.label));
        assert!(row.contains(&f.value), "{}: {row}", f.label);
    }
    for f in &r.sign_up {
        assert!(
            doc.contains(f.label),
            "the sign-up table has no {}",
            f.label
        );
    }
    assert!(doc.contains(r.dashboard));
}

#[test]
fn the_doc_checks_the_command_s_boxes() {
    let doc = doc();
    let r = registration(REDIRECT_PAGE, false);
    let rows: Vec<&str> = doc
        .lines()
        .skip_while(|l| !l.starts_with("| Group on the form |"))
        .skip(2)
        .take_while(|l| l.starts_with('|'))
        .collect();
    assert_eq!(rows.len(), r.scopes.len());
    for (row, group) in rows.iter().zip(&r.scopes) {
        let check = row.split('|').nth(2).unwrap();
        if group.check.len() == PATIENT_TYPES.len() {
            assert!(check.contains("all 27 `patient/<Type>.rs`"), "{row}");
        } else if group.check.is_empty() {
            assert_eq!(check.trim(), "none", "{row}");
        } else {
            let named: Vec<&str> = check.split('`').skip(1).step_by(2).collect();
            assert_eq!(named, group.check, "{row}");
        }
    }
}

#[test]
fn the_doc_lists_the_27_types_in_the_sign_in_s_order() {
    let doc = doc();
    let start = doc.find("The 27 types are ").unwrap() + "The 27 types are ".len();
    let end = start + doc[start..].find(", as `modmed registration`").unwrap();
    let listed: Vec<String> = doc[start..end]
        .replace(" and ", ", ")
        .split(',')
        .map(|t| t.trim().to_string())
        .collect();
    assert_eq!(listed, PATIENT_TYPES);
}
