//! What to enter to register a client: `modmed registration` prints it
//! (`registration/v2`, medical-common's `pk_med_smart::Registration`).
//!
//! modmed has two registrations, one per [`Environment`]:
//! - `production`: an app on ModMed's FHIR vendor dashboard, with the labels
//!   version 3.0.0 of the dashboard shows. docs/registration.md walks
//!   through the same steps, sign-up included, and `tests/registration.rs`
//!   holds the two together.
//! - `test`: a local test server standing in for ModMed (`make mock`),
//!   whose synthetic pair is never ModMed's (`modmed registration --test`).

use pk_cli_auth::vault::Vault;
use pk_cli_core::CliError;
use pk_med_smart::{missing_client, ClientStatus, ClientType, Registration, StoredIn};

use crate::config::Config;
use crate::credentials::client_entry;
use crate::modmed::{scopes, Environment};

/// The vendor dashboard, the **Register** button on ModMed's guide. It signs
/// in through ModMed's Keycloak at `sso.ema.md` (realm `fhir-vendor`), whose
/// page links "Register as New User".
pub const DASHBOARD: &str = "https://fhir-vendor-dashboard.kube.prod.mmicse.com";
/// The step-by-step guide to ModMed's dashboard.
pub const DOC: &str = "https://github.com/piekstra/modmed-cli/blob/main/docs/registration.md";
/// The guide to a test server: `make mock` and its synthetic pair.
pub const TEST_DOC: &str = "https://github.com/piekstra/modmed-cli/blob/main/CONTRIBUTING.md";
/// The https page that passes a sign-in's answer on to modmed's listener
/// (site/callback.html). The dashboard refuses an `http://127.0.0.1`
/// redirect.
pub const REDIRECT_PAGE: &str = "https://piekstra.github.io/modmed-cli/callback";
/// The client is confidential: the secret goes in the token request's body
/// (`client_secret_post`).
pub const CLIENT_TYPE: ClientType = ClientType::ConfidentialSymmetric;
const REPOSITORY: &str = "https://github.com/piekstra/modmed-cli";
const PRIVACY: &str = "https://github.com/piekstra/modmed-cli/blob/main/PRIVACY.md";

/// `environment`'s registration, with the redirect `config` sends.
pub fn registration(environment: Environment, config: &Config) -> Registration {
    match environment {
        Environment::Production => production(config),
        Environment::Test => test(config),
    }
}

fn production(config: &Config) -> Registration {
    let redirect = config.redirect_page.as_deref().unwrap_or(REDIRECT_PAGE);
    let mut reg = Registration::new(
        "modmed",
        "ModMed",
        DASHBOARD,
        DOC,
        CLIENT_TYPE,
        "modmed auth set-credential",
    )
    .registration(Environment::Production.as_str())
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
    .url_with_note(
        "Launch URL",
        REPOSITORY,
        "required once a launch scope is checked; modmed never opens it",
    )
    .url_with_note(
        "Redirect URL",
        redirect,
        "one per app; the form refuses http://127.0.0.1:7470/callback (a 400), so an https page passes the answer on to modmed there",
    )
    .url_with_note("Logo URL", "", "optional; leave empty")
    .url("Policy URL", PRIVACY)
    .url_with_note("Terms of Service URL", "", "optional; leave empty");
    for note in [
        "Sign up first: \"Register as New User\" on the dashboard's sign-in page. The verification link it emails expires within 5 minutes.",
        "The scopes are checkboxes. Under the first Standard Scopes check openid and launch/patient; under Additional Scopes, all 27 Patient Scopes and none of the User Scopes; under the second Standard Scopes, fhirUser and offline_access.",
        "After \"Submit for Approval\" the app is Disabled: choose \"Request to Enable App\", then \"Proceed\". It reads \"Pending for Approval\" until ModMed approves it; ModMed publishes no review time.",
    ] {
        reg = reg.note(note);
    }
    if config.redirect_page.is_none() {
        reg = reg.note(format!(
            "Point modmed at the page: `modmed config set redirect_page {REDIRECT_PAGE}`."
        ));
    }
    reg
}

fn test(config: &Config) -> Registration {
    // A test server stands in for ModMed, so the vendor stays ModMed:
    // missing_client says "no ModMed client is configured for the test
    // registration". The portal is the server, once configured.
    let server = config.test_fhir_base.clone().unwrap_or_default();
    Registration::new(
        "modmed",
        "ModMed",
        server,
        TEST_DOC,
        CLIENT_TYPE,
        "modmed auth set-credential --test",
    )
    .registration(Environment::Test.as_str())
    .command("modmed registration --test")
    .redirect_uri(config.redirect_uri())
    .scopes(&scopes())
    .note("A local test server (`make mock`) has no form: it prints its synthetic pair, which goes in with `modmed auth set-credential --test --stdin`. ModMed's pair never goes to a test server.")
    .note("`modmed config set test_fhir_base <BASE>` names the server, for `--org test`.")
}

/// The error for an environment with no client stored: exit 2, reason
/// `no_client`, its remedy naming the guide, the registration's command and
/// its store command.
pub fn no_client(environment: Environment, config: &Config) -> CliError {
    missing_client(&registration(environment, config))
}

/// `info`'s and `auth status`'s `clients`: one row per registration, from
/// the vault's file names, so the Keychain is never read.
pub fn client_statuses(vault: &Vault, config: &Config) -> Result<Vec<ClientStatus>, CliError> {
    Environment::ALL
        .into_iter()
        .map(|e| {
            let stored = vault.contains(&client_entry(e))?;
            Ok(ClientStatus::of(
                &registration(e, config),
                stored.then_some(StoredIn::Vault),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn the_scopes_listed_are_the_ones_a_sign_in_asks_for() {
        for e in Environment::ALL {
            let mut listed = registration(e, &Config::default()).scopes;
            let mut asked: Vec<String> = crate::modmed::scope()
                .split_whitespace()
                .map(str::to_string)
                .collect();
            listed.sort();
            asked.sort();
            assert_eq!(listed, asked);
            assert_eq!(listed.len(), 31);
        }
    }

    #[test]
    fn the_redirect_is_the_configured_page_and_its_setting_is_named_until_set() {
        let reg = registration(Environment::Production, &Config::default());
        assert_eq!(reg.redirect_uris, [REDIRECT_PAGE]);
        assert!(reg.notes.iter().any(|n| n.ends_with(&format!(
            "`modmed config set redirect_page {REDIRECT_PAGE}`."
        ))));
        let mut own = Config::default();
        own.set("redirect_page", "https://cb.example/modmed/callback")
            .unwrap();
        let reg = registration(Environment::Production, &own);
        assert_eq!(reg.redirect_uris, ["https://cb.example/modmed/callback"]);
        let redirect = reg.urls.iter().find(|u| u.label == "Redirect URL").unwrap();
        assert_eq!(redirect.value, "https://cb.example/modmed/callback");
        assert!(!reg
            .notes
            .iter()
            .any(|n| n.contains("config set redirect_page")));
        // A test server is sent what the config sends.
        let test = registration(Environment::Test, &own);
        assert_eq!(test.redirect_uris, ["https://cb.example/modmed/callback"]);
    }

    #[test]
    fn each_registration_names_itself_and_its_command() {
        let config = Config::default();
        let prod = registration(Environment::Production, &config);
        assert_eq!(prod.registration.as_deref(), Some("production"));
        assert_eq!(prod.command, "modmed registration");
        assert_eq!(prod.doc, DOC);
        let test = registration(Environment::Test, &config);
        assert_eq!(test.registration.as_deref(), Some("test"));
        assert_eq!(test.command, "modmed registration --test");
        assert_eq!(test.store, "modmed auth set-credential --test");
    }

    #[test]
    fn a_missing_client_names_the_guide_the_command_and_the_store() {
        let config = Config::default();
        let err = no_client(Environment::Production, &config);
        assert_eq!(err.exit_code(), 2);
        assert_eq!(
            err.remedy(),
            Some(format!("register one by following {DOC}, entering what `modmed registration` prints, then run `modmed auth set-credential`").as_str())
        );
        assert!(err
            .to_string()
            .starts_with("no ModMed client is configured for the production registration — "));
        assert_eq!(err.to_json()["error"]["reason"], "no_client");
        let test = no_client(Environment::Test, &config);
        assert!(test
            .to_string()
            .starts_with("no ModMed client is configured for the test registration — "));
        assert_eq!(
            test.remedy(),
            Some(format!("register one by following {TEST_DOC}, entering what `modmed registration --test` prints, then run `modmed auth set-credential --test`").as_str())
        );
    }

    #[test]
    fn the_statuses_come_from_the_vault_s_files_without_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let keys = Arc::new(pk_cli_auth::vault::MemoryKeys::new());
        let vault = Vault::new(keys.clone(), dir.path().join("vault"));
        let config = Config::default();
        let none = client_statuses(&vault, &config).unwrap();
        assert!(none.iter().all(|s| !s.configured));
        crate::credentials::store_client(
            &vault,
            Environment::Test,
            &crate::credentials::ClientCredentials::new("client-one", "secret-one").unwrap(),
        )
        .unwrap();
        let reads = keys.reads();
        let statuses = client_statuses(&vault, &config).unwrap();
        assert_eq!(keys.reads(), reads, "the statuses read the key");
        let v = serde_json::to_value(&statuses).unwrap();
        assert_eq!(
            v,
            serde_json::json!([
                {"registration": "production", "configured": false},
                {"registration": "test", "configured": true, "client_type": "confidential-symmetric", "stored_in": "vault"},
            ])
        );
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
