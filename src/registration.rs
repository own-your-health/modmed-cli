//! What to enter on ModMed's FHIR vendor dashboard to register a household's
//! own app: `modmed registration` prints it (`registration/v1`,
//! medical-common's `pk_med_smart::Registration`), with the labels version
//! 3.0.0 of the dashboard shows. docs/registration.md walks through the same
//! steps, sign-up included; `tests/registration.rs` holds the two together.

use pk_cli_core::CliError;
use pk_med_smart::{missing_client, ClientStatus, ClientType, Registration, StoredIn};

use crate::modmed::{scopes, Environment};

/// The vendor dashboard, the **Register** button on ModMed's guide. It signs
/// in through ModMed's Keycloak at `sso.ema.md` (realm `fhir-vendor`), whose
/// page links "Register as New User".
pub const DASHBOARD: &str = "https://fhir-vendor-dashboard.kube.prod.mmicse.com";
/// The step-by-step guide, as `registration/v1` names it: its path in the
/// repository.
pub const DOC: &str = "docs/registration.md";
/// The https page that passes a sign-in's answer on to modmed's listener
/// (site/callback.html). The dashboard refuses an `http://127.0.0.1`
/// redirect.
pub const REDIRECT_PAGE: &str = "https://piekstra.github.io/modmed-cli/callback";
/// The app is a confidential client: the secret goes in the token request's
/// body (`client_secret_post`).
pub const CLIENT_TYPE: ClientType = ClientType::ConfidentialSymmetric;
const REPOSITORY: &str = "https://github.com/piekstra/modmed-cli";
const PRIVACY: &str = "https://github.com/piekstra/modmed-cli/blob/main/PRIVACY.md";

/// The command that stores the pair the dashboard shows.
fn store(environment: Environment) -> &'static str {
    match environment {
        Environment::Production => "modmed auth set-credential",
        Environment::Test => "modmed auth set-credential --test",
    }
}

/// The values for an app whose redirect is `redirect`: the configured
/// `redirect_page`, else [`REDIRECT_PAGE`]. `redirect_set` says whether
/// modmed already sends it, so the step that sets it is left out.
pub fn registration(redirect: &str, redirect_set: bool) -> Registration {
    let mut reg = Registration::new(
        "modmed",
        "ModMed",
        DASHBOARD,
        DOC,
        CLIENT_TYPE,
        store(Environment::Production),
    )
    .redirect_uri(redirect)
    .scopes(&scopes())
    .field_with_note(
        "Application Name",
        "modmed",
        "required; any name your household recognises, which the practice's sign-in may show",
    )
    .field_with_note(
        "Description",
        "Downloads the patient's own records to their own computer. Reads only.",
        "optional",
    )
    .field("App Type", "Patient")
    .field_with_note("FHIR Version", "v4.0.1", "shown, not chosen")
    .field("Access Type", "Client-Confidential")
    .field("PKCE", "Use PKCE S256")
    .url("Launch URL", REPOSITORY)
    .url("Redirect URL", redirect)
    .url("Logo URL", "")
    .url("Policy URL", PRIVACY)
    .url("Terms of Service URL", "");
    for note in [
        "Sign up first: \"Register as New User\" on the dashboard's sign-in page. The verification link it emails expires within 5 minutes.",
        "The Launch URL is required once a launch scope is checked (\"Base (Launch) Url must be a valid url if a launch scope is present.\"); modmed never opens it. Logo URL and Terms of Service URL stay empty.",
        "The form takes one Redirect URL and refuses http://127.0.0.1:7470/callback (a 400), so the app registers an https page that passes the sign-in's answer on to modmed at that address.",
        "The scopes are checkboxes. Under the first Standard Scopes check openid and launch/patient; under Additional Scopes, all 27 Patient Scopes and none of the User Scopes; under the second Standard Scopes, fhirUser and offline_access.",
        "After \"Submit for Approval\" the app is Disabled: choose \"Request to Enable App\", then \"Proceed\". It reads \"Pending for Approval\" until ModMed approves it; ModMed publishes no review time.",
    ] {
        reg = reg.note(note);
    }
    if !redirect_set {
        reg = reg.note(format!(
            "Point modmed at the page: `modmed config set redirect_page {redirect}`."
        ));
    }
    reg
}

/// The error for an environment with no client stored: exit 2, reason
/// `no-client`, its remedy naming the guide, `modmed registration` and the
/// store command.
pub fn no_client(environment: Environment) -> CliError {
    let mut reg = registration(REDIRECT_PAGE, true);
    reg.store = store(environment).into();
    missing_client(&reg, Some(environment.as_str()))
}

/// `info`'s and `auth status`'s `client` member: the production client when
/// it is stored, else the test one, else none.
pub fn client_status(stored: impl Fn(Environment) -> bool) -> ClientStatus {
    Environment::ALL
        .into_iter()
        .find(|e| stored(*e))
        .map(|e| ClientStatus::configured(CLIENT_TYPE, StoredIn::Vault).registration(e.as_str()))
        .unwrap_or_else(ClientStatus::none)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scopes_listed_are_the_ones_a_sign_in_asks_for() {
        let mut listed = registration(REDIRECT_PAGE, false).scopes;
        let mut asked: Vec<String> = crate::modmed::scope()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        listed.sort();
        asked.sort();
        assert_eq!(listed, asked);
        assert_eq!(listed.len(), 31);
    }

    #[test]
    fn the_redirect_is_the_configured_page_and_its_setting_is_named_until_set() {
        let reg = registration(REDIRECT_PAGE, false);
        assert_eq!(reg.redirect_uris, [REDIRECT_PAGE]);
        assert!(reg.notes.iter().any(|n| n.ends_with(&format!(
            "`modmed config set redirect_page {REDIRECT_PAGE}`."
        ))));
        let own = registration("https://cb.example/modmed/callback", true);
        assert_eq!(own.redirect_uris, ["https://cb.example/modmed/callback"]);
        let redirect = own.urls.iter().find(|u| u.label == "Redirect URL").unwrap();
        assert_eq!(redirect.value, "https://cb.example/modmed/callback");
        assert!(!own.notes.iter().any(|n| n.contains("config set")));
    }

    #[test]
    fn a_missing_client_names_the_guide_the_command_and_the_store() {
        let err = no_client(Environment::Production);
        assert_eq!(err.exit_code(), 2);
        assert_eq!(
            err.remedy(),
            Some("register one as docs/registration.md describes, entering what `modmed registration` prints, then run `modmed auth set-credential`")
        );
        assert!(err.to_string().contains("for the production registration"));
        let test = no_client(Environment::Test);
        assert!(test
            .remedy()
            .unwrap()
            .ends_with("then run `modmed auth set-credential --test`"));
    }

    #[test]
    fn the_status_is_the_production_client_first() {
        assert!(!client_status(|_| false).configured);
        let test = client_status(|e| e == Environment::Test);
        assert_eq!(test.registration.as_deref(), Some("test"));
        let both = client_status(|_| true);
        assert_eq!(both.registration.as_deref(), Some("production"));
        assert_eq!(both.client_type, Some(CLIENT_TYPE));
        assert_eq!(both.stored_in, Some(StoredIn::Vault));
        assert_eq!(both.client_id, None);
    }

    #[test]
    fn the_page_is_the_one_the_site_publishes() {
        let site =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/site/callback.html"))
                .unwrap();
        assert!(REDIRECT_PAGE.starts_with("https://piekstra.github.io/modmed-cli/"));
        assert!(site.contains("http://127.0.0.1:7470/callback"));
    }
}
