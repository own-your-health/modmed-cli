//! What to enter on ModMed's FHIR vendor dashboard to register a household's
//! own app (`modmed registration`, `registration/v1`), with the labels
//! version 3.0.0 of the dashboard shows. docs/registration.md walks through
//! the same steps; this is the copy a program can read.

use serde::Serialize;

use crate::modmed::{CONTEXT_SCOPES, PATIENT_TYPES};

/// The vendor dashboard. It signs in through ModMed's Keycloak at
/// `sso.ema.md` (realm `fhir-vendor`), whose page links "Register as New
/// User".
pub const DASHBOARD: &str = "https://fhir-vendor-dashboard.kube.prod.mmicse.com";
/// The step-by-step guide.
pub const DOC: &str = "https://github.com/piekstra/modmed-cli/blob/main/docs/registration.md";
/// The https page that passes a sign-in's answer on to modmed's listener
/// (site/callback.html). The dashboard refuses an `http://127.0.0.1`
/// redirect.
pub const REDIRECT_PAGE: &str = "https://piekstra.github.io/modmed-cli/callback";
const REPOSITORY: &str = "https://github.com/piekstra/modmed-cli";
const PRIVACY: &str = "https://github.com/piekstra/modmed-cli/blob/main/PRIVACY.md";

/// One field of a form: its label as the page shows it, what to enter, and
/// why when it is not obvious.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Field {
    pub label: &'static str,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

/// One group of scope checkboxes on the app form, under its heading, with
/// the boxes to check.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ScopeGroup {
    pub heading: &'static str,
    pub check: Vec<String>,
    /// Boxes in the group to leave unchecked.
    pub leave: Vec<&'static str>,
}

/// `registration/v1`'s payload.
#[derive(Debug, Clone, Serialize)]
pub struct Registration {
    pub vendor: &'static str,
    pub dashboard: &'static str,
    pub doc: &'static str,
    /// The "Register" page linked from the sign-in.
    pub sign_up: Vec<Field>,
    /// "Register New App", the "App Details" form.
    pub app: Vec<Field>,
    pub scopes: Vec<ScopeGroup>,
    /// What to do after "Submit for Approval", in order.
    pub then: Vec<String>,
}

fn field(label: &'static str, value: &str, note: Option<&'static str>) -> Field {
    Field {
        label,
        value: value.to_string(),
        note,
    }
}

/// The values for an app whose redirect is `redirect` (the configured
/// `redirect_page`, else [`REDIRECT_PAGE`]); `redirect_set` says whether
/// modmed already sends it.
pub fn registration(redirect: &str, redirect_set: bool) -> Registration {
    let own = "Your own.";
    let sign_up = vec![
        field("First Name", own, None),
        field("Last Name", own, None),
        field(
            "Email",
            "Your own address.",
            Some("The verification link it is sent expires within 5 minutes."),
        ),
        field(
            "Company Name",
            "Your own name.",
            Some("A household registering its own app is not a company."),
        ),
        field("Web page", REPOSITORY, None),
        field("Street", own, None),
        field("City or Locality", own, None),
        field("State, Province, or Region", own, None),
        field("Zip or Postal code", own, None),
        field("Country", own, None),
        field("Phone number", "Optional; may be left empty.", None),
        field(
            "Description of Company or Applications",
            "An individual using the open-source modmed command-line program to download my own records from my ModMed practice to my own computer.",
            None,
        ),
        field("Password", "A new password, kept in your password manager.", None),
        field("Confirm Password", "The same password.", None),
    ];
    let app = vec![
        field(
            "Application Name",
            "modmed",
            Some("Required. Any name your household recognises; the practice's consent page may show it."),
        ),
        field(
            "Description",
            "Downloads the patient's own records to their own computer. Reads only.",
            Some("Optional."),
        ),
        field("App Type", "Patient", None),
        field("FHIR Version", "v4.0.1", Some("Shown, not chosen.")),
        field(
            "Access Type",
            "Client-Confidential",
            Some("modmed sends the client secret in the token request's body."),
        ),
        field("PKCE", "Use PKCE S256", None),
        field(
            "Launch URL",
            REPOSITORY,
            Some("Required once launch/patient is checked; any https address of the app."),
        ),
        field(
            "Redirect URL",
            redirect,
            Some("One per app. The dashboard refuses http://127.0.0.1:7470/callback (a 400), so this https page passes the answer on to it."),
        ),
        field("Logo URL", "", Some("Optional; leave empty.")),
        field("Policy URL", PRIVACY, None),
        field("Terms of Service URL", "", Some("Optional; leave empty.")),
    ];
    let pick = |names: &[&str]| -> Vec<String> {
        CONTEXT_SCOPES
            .iter()
            .filter(|s| names.contains(s))
            .map(|s| s.to_string())
            .collect()
    };
    let scopes = vec![
        ScopeGroup {
            heading: "Standard Scopes",
            check: pick(&["openid", "launch/patient"]),
            leave: vec!["online_access", "launch", "launch/encounter"],
        },
        ScopeGroup {
            heading: "Additional Scopes: Patient Scopes",
            check: PATIENT_TYPES
                .iter()
                .map(|t| format!("patient/{t}.rs"))
                .collect(),
            leave: vec![],
        },
        ScopeGroup {
            heading: "Additional Scopes: User Scopes",
            check: vec![],
            leave: vec!["every user/*.rs"],
        },
        ScopeGroup {
            heading: "Standard Scopes (the second group)",
            check: pick(&["fhirUser", "offline_access"]),
            leave: vec!["profile"],
        },
    ];
    let mut then = vec![
        "Choose \"Submit for Approval\". The app's row reads Status \"Disabled\".".to_string(),
        "Choose its action \"Request to Enable App\", then \"Proceed\". The row reads \"Pending for Approval\" until ModMed approves it.".to_string(),
    ];
    then.push(
        "Store the Client ID and Secret the row shows: `modmed auth set-credential` (asks for each without echoing it), or `--stdin` with `<id>:<secret>` or the two on separate lines.".to_string(),
    );
    if !redirect_set {
        then.push(format!(
            "Point modmed at the redirect page: `modmed config set redirect_page {redirect}`"
        ));
    }
    Registration {
        vendor: "modmed",
        dashboard: DASHBOARD,
        doc: DOC,
        sign_up,
        app,
        scopes,
        then,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_boxes_to_check_are_exactly_the_scopes_a_sign_in_asks_for() {
        let r = registration(REDIRECT_PAGE, false);
        let checked: BTreeSet<String> = r.scopes.iter().flat_map(|g| g.check.clone()).collect();
        let asked: BTreeSet<String> = crate::modmed::scope()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        assert_eq!(checked, asked);
        assert_eq!(checked.len(), 31);
    }

    #[test]
    fn the_redirect_is_the_page_and_its_setting_is_named_until_set() {
        let r = registration(REDIRECT_PAGE, false);
        let redirect = r.app.iter().find(|f| f.label == "Redirect URL").unwrap();
        assert_eq!(redirect.value, REDIRECT_PAGE);
        assert!(r.then.iter().any(|s| s.ends_with(&format!(
            "`modmed config set redirect_page {REDIRECT_PAGE}`"
        ))));
        let set = registration(REDIRECT_PAGE, true);
        assert!(!set.then.iter().any(|s| s.contains("config set")));
    }

    #[test]
    fn the_page_is_the_one_the_site_publishes() {
        let site =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/site/callback.html"))
                .unwrap();
        let path = REDIRECT_PAGE
            .strip_prefix("https://piekstra.github.io/modmed-cli/")
            .unwrap();
        assert_eq!(path, "callback");
        assert!(site.contains("http://127.0.0.1:7470/callback"));
    }
}
