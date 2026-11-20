//! Sign-in, renewal, export, documents, `api` and logout in-process against
//! medical-common's mock SMART + FHIR server on 127.0.0.1, standing in for
//! a ModMed practice as the `test` organization. Sessions and client
//! credentials live in a vault over a temporary directory with an
//! in-memory key; nothing touches the Keychain or the network beyond
//! 127.0.0.1. Every value is the mock's synthetic patient, signed in under
//! the profile `adult1`.

mod support;

use std::any::Any;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use pk_cli_auth::refresh_lock::RefreshLock;
use pk_cli_auth::vault::{MemoryKeys, Vault};
use pk_cli_config::profile::ProfileName;
use pk_cli_config::ConfigStore;
use pk_med_fhir::test_util::{http_get, Fault, MockServer, CLIENT_ID, CLIENT_SECRET, PATIENT_ID};
use pk_med_records::{Item, RecordsReader};
use pk_med_smart::SessionStore;

use modmed::cli::{
    AuthCmd, DocumentsCmd, ExportArgs, LoginArgs, LogoutArgs, PatientArgs, RecordsCmd,
};
use modmed::commands::{self, Ctx};
use modmed::config::{Config, ProfileSettings};
use modmed::credentials::{self, ClientCredentials};
use modmed::modmed::Environment;

/// The browser's part: follow the authorization URL to its redirect and
/// deliver the redirect to modmed's loopback listener, on another thread.
/// It opens for the profile the command runs as.
fn window() -> modmed::commands::OpenWindow {
    Box::new(|profile: &ProfileName, url: &str| {
        assert_eq!(profile.as_str(), "adult1");
        let url = url.to_string();
        std::thread::spawn(move || {
            let answer = http_get(&url).expect("the mock answers the authorization");
            assert_eq!(answer.status, 302, "{}", answer.body);
            let back = answer.header("location").unwrap().to_string();
            http_get(&back).expect("modmed's listener answers the redirect");
        });
        Ok(Box::new(()) as Box<dyn Any>)
    })
}

fn adult1() -> ProfileName {
    ProfileName::parse("adult1").unwrap()
}

/// The patients `adult1` signed in.
fn settings(ctx: &Ctx) -> ProfileSettings {
    ctx.config().unwrap().profiles.settings(&adult1()).unwrap()
}

fn save(ctx: &Ctx, settings: &ProfileSettings) {
    ctx.save_settings(&adult1(), settings).unwrap();
}

/// The vault entry of a sign-in's account in `adult1`.
fn entry(account: &str) -> String {
    format!("adult1/{}", account.strip_prefix("smart/").unwrap())
}

const ACCOUNT: &str = "smart/test/pat-0001";

fn ctx(dir: &Path, mock: &MockServer) -> Ctx {
    let store = ConfigStore::new("modmed").with_override(Some(dir.join("config.json")));
    let mut config = Config::default();
    config.set("test_fhir_base", &mock.fhir_base()).unwrap();
    // Any directory read fails on this closed port, never reaching ModMed.
    config
        .set("directory_url", "http://127.0.0.1:9/fhir/r4/Endpoint")
        .unwrap();
    config.profiles.add(adult1());
    store.save(&config).unwrap();
    let ctx = Ctx {
        common: pk_cli_core::CommonArgs {
            quiet: true,
            ..Default::default()
        },
        store,
        profile: Some("adult1".into()),
        vault: Arc::new(Vault::new(MemoryKeys::new(), dir.join("vault"))),
        lock: RefreshLock::at(dir.join("refresh.lock")),
        cache: dir.join("cache"),
        window: window(),
        forget_browser: Box::new(|_| Ok(false)),
        any_port: true,
    };
    credentials::store_client(
        &ctx.vault,
        Environment::Test,
        &ClientCredentials::new(CLIENT_ID, CLIENT_SECRET).unwrap(),
    )
    .unwrap();
    ctx
}

fn login_args(org: &str, label: Option<&str>) -> LoginArgs {
    LoginArgs {
        org: org.into(),
        label: label.map(Into::into),
        no_browser: false,
        timeout: 1,
    }
}

fn login(ctx: &Ctx, label: &str) {
    commands::auth::run(ctx, &AuthCmd::Login(login_args("test", Some(label)))).unwrap();
}

fn export_args(output: Option<&Path>, since: Option<&str>) -> ExportArgs {
    ExportArgs {
        who: PatientArgs::default(),
        since: since.map(Into::into),
        ndjson: output.is_none(),
        output: output.map(Path::to_path_buf),
    }
}

/// Export to `dir`: the counts by type, and each skip as `code what`.
fn export(ctx: &Ctx, dir: &Path, since: Option<&str>) -> (BTreeMap<String, u64>, Vec<String>) {
    let failed = commands::records::export(ctx, &export_args(Some(dir), since)).unwrap();
    assert!(failed.is_none(), "{failed:?}");
    let file = std::fs::File::open(dir.join("records.ndjson")).unwrap();
    let mut reader = RecordsReader::new(std::io::BufReader::new(file));
    let header = reader.header().unwrap().clone();
    assert_eq!(header.organization.id, "test");
    assert_eq!(header.organization.vendor.as_deref(), Some("modmed"));
    assert_eq!(header.patient.id, PATIENT_ID);
    assert_eq!(header.patient.label.as_deref(), Some("adult1"));
    assert_eq!(header.patient.kind.as_deref(), Some("patient"));
    assert_eq!(header.source.cli, "modmed");
    let mut skipped = Vec::new();
    for item in reader.by_ref() {
        if let Item::Trailer(t) = item.unwrap() {
            assert!(t.complete);
            skipped = t
                .skipped
                .iter()
                .map(|s| {
                    format!(
                        "{} {}",
                        s.code,
                        s.search
                            .clone()
                            .or(s.resource_type.clone())
                            .unwrap_or_default()
                    )
                })
                .collect();
        }
    }
    (reader.counts().clone(), skipped)
}

fn count_of(mock: &MockServer, path: &str) -> usize {
    mock.requests().iter().filter(|r| r.path() == path).count()
}

#[test]
fn sign_in_then_export_everything_the_patient_has() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");

    let state = settings(&ctx);
    assert_eq!(state.grants.len(), 1);
    assert_eq!(state.grants[0].id, PATIENT_ID);
    assert_eq!(state.grants[0].organization, "test");
    // The session is the profile's vault entry beside the shared client.
    assert_eq!(
        ctx.vault.names().unwrap(),
        ["_client/test".to_string(), entry(ACCOUNT)]
    );

    // Discovery read the SMART configuration at the base, and the token
    // request carried the client's id and secret in the body
    // (client_secret_post), with no Basic header.
    let seen = mock.requests();
    assert_eq!(seen[0].path(), "/fhir/.well-known/smart-configuration");
    let token = seen.iter().find(|r| r.path() == "/token").unwrap();
    assert!(token.header("authorization").is_none());
    assert!(
        token.body.contains("client_id=synthetic-client"),
        "{}",
        token.body
    );
    assert!(token.body.contains("client_secret=synthetic-secret"));
    let authorize = seen
        .iter()
        .find(|r| r.path() == "/authorize")
        .map(|r| r.target.clone())
        .unwrap();
    for want in [
        "launch%2Fpatient",
        "offline_access",
        "patient%2FDocumentReference.rs",
        "code_challenge_method=S256",
        "aud=",
    ] {
        assert!(authorize.contains(want), "{want}: {authorize}");
    }

    let out = dir.path().join("out");
    let (counts, skipped) = export(&ctx, &out, None);
    let want: BTreeMap<String, u64> = [
        ("AllergyIntolerance", 1),
        ("Binary", 1),
        ("Condition", 1),
        ("Coverage", 1),
        ("DiagnosticReport", 1),
        ("DocumentReference", 1),
        ("Encounter", 1),
        ("Immunization", 1),
        ("MedicationRequest", 1),
        ("Observation", 3),
        ("Organization", 2),
        ("Patient", 1),
        ("Practitioner", 1),
        ("Procedure", 1),
    ]
    .into_iter()
    .map(|(t, n)| (t.to_string(), n))
    .collect();
    assert_eq!(counts, want);
    assert!(skipped.is_empty(), "{skipped:?}");

    // Every search was by patient.
    for r in mock.requests() {
        let Some(t) = r.path().strip_prefix("/fhir/") else {
            continue;
        };
        if modmed::export::SEARCHES
            .iter()
            .any(|s| s.resource_type == t)
        {
            assert!(
                r.target.contains(&format!("patient={PATIENT_ID}")),
                "{t}: {}",
                r.target
            );
        }
    }
    // The Binary follows the first resource naming it (the mock's
    // DiagnosticReport presents the same file its DocumentReference holds).
    let text = std::fs::read_to_string(out.join("records.ndjson")).unwrap();
    let types: Vec<String> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v["resourceType"].as_str().map(str::to_string))
        .collect();
    let binary = types.iter().position(|t| t == "Binary").unwrap();
    assert_eq!(types[binary - 1], "DiagnosticReport", "{types:?}");
}

#[test]
fn since_bounds_the_dated_searches_and_reads_the_rest_whole() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    let out = dir.path().join("out");
    export(&ctx, &out, Some("2024-01-01"));
    let target = |path: &str| {
        mock.requests()
            .into_iter()
            .find(|r| r.path() == path)
            .unwrap()
            .target
    };
    assert!(target("/fhir/Observation").contains("date=ge2024-01-01"));
    assert!(target("/fhir/MedicationRequest").contains("authoredon=ge2024-01-01"));
    assert!(target("/fhir/ServiceRequest").contains("authored=ge2024-01-01"));
    assert!(!target("/fhir/Condition").contains("2024-01-01"));
    let text = std::fs::read_to_string(out.join("records.ndjson")).unwrap();
    let header: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(header["since"], "2024-01-01");
    let unbounded = header["since_unbounded"].as_array().unwrap();
    for t in ["Patient", "Condition", "Medication"] {
        assert!(unbounded.contains(&t.into()), "{t}");
    }
    assert!(!unbounded.contains(&"Observation".into()));
}

#[test]
fn a_search_refused_as_a_bad_request_is_asked_again_by_category() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    mock.fail(Fault::not_supported("/fhir/Observation").times(1));
    let out = dir.path().join("out");
    let (counts, skipped) = export(&ctx, &out, None);
    assert!(skipped.is_empty(), "{skipped:?}");
    assert_eq!(counts["Observation"], 3);
    let by_category: Vec<String> = mock
        .requests()
        .into_iter()
        .filter(|r| r.path() == "/fhir/Observation")
        .map(|r| r.target)
        .filter(|t| t.contains("category=") && !t.contains("_page="))
        .collect();
    assert_eq!(
        by_category.len(),
        modmed::export::OBSERVATION_CATEGORIES.len()
    );
    assert!(by_category[0].contains("category=laboratory"));
}

#[test]
fn a_type_with_no_split_refused_as_a_bad_request_is_one_skip() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    mock.fail(Fault::not_supported("/fhir/Condition").times(1));
    let (counts, skipped) = export(&ctx, &dir.path().join("out"), None);
    assert!(!counts.contains_key("Condition"));
    assert_eq!(skipped, ["upstream Condition"]);
    assert_eq!(count_of(&mock, "/fhir/Condition"), 1);
}

#[test]
fn a_refused_token_is_renewed_once_and_the_export_goes_on() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    mock.fail(Fault::unauthorized("/fhir/Condition").times(1));
    let (counts, _) = export(&ctx, &dir.path().join("out"), None);
    assert_eq!(counts["Condition"], 1);
    let refreshes = mock
        .requests()
        .iter()
        .filter(|r| r.path() == "/token" && r.body.contains("grant_type=refresh_token"))
        .count();
    assert_eq!(refreshes, 1);
    // The refresh carried the secret in the body too.
    let refresh = mock
        .requests()
        .into_iter()
        .find(|r| r.path() == "/token" && r.body.contains("grant_type=refresh_token"))
        .unwrap();
    assert!(refresh.body.contains("client_secret="));
}

#[test]
fn a_refused_search_is_a_skip_not_a_stop() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    mock.fail(Fault::new(
        "GET",
        "/fhir/Encounter",
        403,
        r#"{"resourceType":"OperationOutcome","issue":[{"severity":"error","code":"forbidden","diagnostics":"scope not granted"}]}"#,
    ));
    mock.fail(Fault::unavailable("/fhir/MedicationDispense").times(5));
    mock.fail(Fault::new("GET", "/fhir/Binary/", 404, "").times(5));
    let (counts, skipped) = export(&ctx, &dir.path().join("out"), None);
    assert!(!counts.contains_key("Encounter"));
    assert!(!counts.contains_key("Binary"));
    assert_eq!(counts["Condition"], 1);
    for want in [
        "forbidden Encounter",
        "upstream MedicationDispense",
        "not_found Binary",
    ] {
        assert!(skipped.contains(&want.to_string()), "{want}: {skipped:?}");
    }
}

#[test]
fn results_a_search_says_it_left_out_are_a_skip_and_the_rest_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    let line = "Client not authorized for Condition - Synthetic Sub-resource";
    mock.withhold("Condition", line);
    let out = dir.path().join("out");
    let (counts, skipped) = export(&ctx, &out, None);
    assert_eq!(counts["Condition"], 1);
    assert_eq!(skipped, ["forbidden Condition"]);
    let written = std::fs::read_to_string(out.join("records.ndjson")).unwrap();
    let trailer: serde_json::Value = serde_json::from_str(written.lines().last().unwrap()).unwrap();
    let skip = &trailer["skipped"][0];
    assert_eq!(skip["resource_type"], "Condition", "{skip}");
    assert_eq!(skip["reason"], line, "{skip}");
    assert!(skip.get("search").is_none(), "{skip}");
    assert!(skip.get("id").is_none(), "{skip}");
}

#[test]
fn a_missing_sign_in_names_modmeds_command_once() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    assert!(ctx.sessions(&adult1()).delete(ACCOUNT).unwrap());
    let err = commands::records::export(&ctx, &export_args(None, None)).unwrap_err();
    assert_eq!(err.exit_code(), 3, "{err}");
    assert!(!err.message().contains("auth login"), "{err}");
    assert_eq!(
        err.remedy(),
        Some("run `modmed auth login --profile adult1 --org test --label adult1`")
    );
}

#[test]
fn a_sign_in_a_newer_modmed_stored_says_to_update() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    let stored = ctx.vault.get(&entry(ACCOUNT)).unwrap().unwrap();
    let mut session: serde_json::Value = serde_json::from_str(stored.expose()).unwrap();
    session["format"] = (pk_med_smart::SESSION_FORMAT + 1).into();
    ctx.vault
        .put(
            &entry(ACCOUNT),
            &pk_cli_secrets::Secret::new(session.to_string().as_str()),
        )
        .unwrap();
    let err = commands::records::export(&ctx, &export_args(Some(&dir.path().join("out")), None))
        .unwrap_err();
    assert_eq!(err.remedy(), Some("run `modmed self-update`"), "{err}");
    assert!(!err.to_string().contains(PATIENT_ID), "{err}");
    let list = commands::patients::list(&ctx, true, None).unwrap();
    assert_eq!(list["items"][0]["remedy"], "run `modmed self-update`");
}

#[test]
fn a_practices_patient_is_not_written_to_a_plain_file() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    let mut state = settings(&ctx);
    state.upsert(modmed::session::Context {
        id: PATIENT_ID.into(),
        organization: "example.mmi".into(),
        organization_name: Some("Example Dermatology".into()),
        label: None,
        relationship: None,
        fhir_user: None,
        signed_in_at: "2020-01-01T00:00:00Z".into(),
    });
    save(&ctx, &state);
    let err = commands::records::run(
        &ctx,
        &RecordsCmd::Export(export_args(Some(&dir.path().join("out")), None)),
    )
    .unwrap_err();
    assert_eq!(err.exit_code(), 2, "{err}");
    let remedy = err.remedy().unwrap_or_default();
    assert!(!err.to_string().contains(PATIENT_ID), "{err}");
    assert!(!remedy.contains(PATIENT_ID), "{remedy}");
    assert!(
        remedy.contains(
            "medrec --profile <FOLDER> import --from modmed --from-profile adult1 --patient <WHO>"
        ),
        "{remedy}"
    );
    assert!(mock.requests().is_empty());
}

#[test]
fn a_declined_consent_stores_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    mock.deny_next_authorization();
    let err = commands::auth::run(&ctx, &AuthCmd::Login(login_args("test", None))).unwrap_err();
    assert_eq!(err.exit_code(), 3, "{err}");
    assert_eq!(
        err.remedy(),
        Some("run `modmed auth login --profile adult1 --org test --timeout 1` again")
    );
    assert!(settings(&ctx).grants.is_empty());
    assert!(ctx.sessions(&adult1()).accounts().unwrap().is_empty());
}

#[test]
fn a_second_patient_needs_patient_and_logout_removes_one() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    // The mock serves one patient; a second row stands for a child signed
    // in through the parent's portal account.
    let mut state = settings(&ctx);
    let mut other = state.grants[0].clone();
    other.id = "pat-0002".into();
    other.label = Some("child1".into());
    state.upsert(other);
    save(&ctx, &state);
    let err = commands::records::run(
        &ctx,
        &RecordsCmd::Export(export_args(Some(&dir.path().join("out")), None)),
    )
    .unwrap_err();
    assert_eq!(err.exit_code(), 2);
    commands::auth::run(
        &ctx,
        &AuthCmd::Logout(LogoutArgs {
            who: PatientArgs {
                patient: Some("adult1".into()),
                org: None,
            },
            forget: false,
        }),
    )
    .unwrap();
    let left = settings(&ctx).grants;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].label.as_deref(), Some("child1"));
    assert!(ctx.vault.get(&entry(ACCOUNT)).unwrap().is_none());
}

#[test]
fn set_credential_then_status_reads_both() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    // The pair goes in as `set-credential` reads it, without setting a
    // variable while other tests run.
    commands::auth::store_pair(
        &ctx,
        Environment::Production,
        false,
        Some(
            ClientCredentials::parse_pair(&pk_cli_secrets::Secret::new("client-one:secret-one"))
                .unwrap(),
        ),
    )
    .unwrap();
    let production = credentials::read_client(&ctx.vault, Environment::Production)
        .unwrap()
        .unwrap();
    assert_eq!(production.client_id, "client-one");
    assert_eq!(production.secret().expose(), "secret-one");
    // The test pair `ctx` stored is untouched.
    let test = credentials::read_client(&ctx.vault, Environment::Test)
        .unwrap()
        .unwrap();
    assert_eq!(test.client_id, CLIENT_ID);
    let (_, status) = commands::auth::status_of(&ctx).unwrap();
    assert_eq!(
        status["clients"],
        serde_json::json!([
            {"environment": "production", "stored": true},
            {"environment": "test", "stored": true}
        ])
    );
}

#[test]
fn a_practice_signs_in_with_the_production_client_only() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let directory = support::directory::DirectoryServer::start();
    let ctx = ctx(dir.path(), &mock);
    let mut config = ctx.config().unwrap();
    config.set("directory_url", &directory.url()).unwrap();
    ctx.save_config(&config).unwrap();
    // The test client is stored, the production one is not: the practice
    // is refused before its server is asked anything.
    let err = commands::auth::run(
        &ctx,
        &AuthCmd::Login(login_args("exampledermatology", None)),
    )
    .unwrap_err();
    assert_eq!(err.exit_code(), 3, "{err}");
    assert_eq!(
        err.remedy(),
        Some("register your own app with ModMed (`modmed registration` prints the form's values; https://github.com/piekstra/modmed-cli/blob/main/docs/registration.md), then run `modmed auth set-credential`")
    );
    assert!(mock.requests().is_empty());
    // A practice the directory does not list is not found, with the search
    // as the remedy.
    let err = commands::auth::run(&ctx, &AuthCmd::Login(login_args("nowhere", None))).unwrap_err();
    assert_eq!(err.exit_code(), 4, "{err}");
    assert!(err.remedy().unwrap().contains("modmed orgs search"));
}

#[test]
fn a_stored_sign_in_pointing_elsewhere_is_refused_before_any_renewal() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    let sessions = ctx.sessions(&adult1());
    let mut session = sessions.load(ACCOUNT).unwrap().unwrap();
    session.token_endpoint = "https://mock.example:8080/token".into();
    session.tokens.expires_at = Some(1);
    sessions.save(&session).unwrap();
    let tokens_before = count_of(&mock, "/token");
    let err = commands::records::export(&ctx, &export_args(Some(&dir.path().join("out")), None))
        .unwrap_err();
    assert_eq!(err.exit_code(), 2, "{err}");
    let err = commands::api::run(
        &ctx,
        &modmed::cli::ApiArgs {
            method: "GET".into(),
            path: "Condition?patient=pat-0001".into(),
            who: PatientArgs::default(),
        },
    )
    .unwrap_err();
    assert_eq!(err.exit_code(), 2, "{err}");
    assert_eq!(count_of(&mock, "/token"), tokens_before);
}

#[test]
fn a_sign_in_through_a_forwarding_page_reaches_the_listener_and_names_the_page() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let mut ctx = ctx(dir.path(), &mock);
    let page = "https://cb.example/modmed-cli/callback";
    mock.accept_redirect(page);
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut config = ctx.config().unwrap();
    config.set("redirect_page", page).unwrap();
    config.set("redirect_port", &port.to_string()).unwrap();
    ctx.save_config(&config).unwrap();
    // The browser lands on the page, which passes the query on to the
    // listener unchanged (site/callback.html).
    ctx.window = Box::new(move |_: &ProfileName, url: &str| {
        let url = url.to_string();
        std::thread::spawn(move || {
            let answer = http_get(&url).expect("the mock answers the authorization");
            assert_eq!(answer.status, 302, "{}", answer.body);
            let back = answer.header("location").unwrap().to_string();
            let query = back
                .strip_prefix(page)
                .expect("the practice redirects to the page");
            http_get(&format!("http://127.0.0.1:{port}/callback{query}"))
                .expect("modmed's listener answers the forwarded redirect");
        });
        Ok(Box::new(()) as Box<dyn Any>)
    });
    login(&ctx, "adult1");
    let authorize = mock
        .requests()
        .into_iter()
        .find(|r| r.path() == "/authorize")
        .unwrap();
    assert!(
        authorize
            .target
            .contains("redirect_uri=https%3A%2F%2Fcb.example%2Fmodmed-cli%2Fcallback"),
        "{}",
        authorize.target
    );
    // The mock refuses an exchange whose redirect_uri is not the page.
    let token = mock
        .requests()
        .into_iter()
        .find(|r| r.path() == "/token")
        .unwrap();
    assert!(
        token
            .body
            .contains("redirect_uri=https%3A%2F%2Fcb.example%2Fmodmed-cli%2Fcallback"),
        "{}",
        token.body
    );
    assert_eq!(settings(&ctx).grants.len(), 1);
}

#[test]
fn api_reads_under_the_practice_s_fhir_base_only() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    let api = |path: &str| {
        commands::api::run(
            &ctx,
            &modmed::cli::ApiArgs {
                method: "GET".into(),
                path: path.into(),
                who: PatientArgs::default(),
            },
        )
    };
    api("Condition?patient=pat-0001").unwrap();
    api(&format!("{}/Patient/pat-0001", mock.fhir_base())).unwrap();
    // A token search's system is a URL inside a relative path.
    api("Condition?patient=pat-0001&code=http://snomed.info/sct|123").unwrap();
    assert!(mock
        .requests()
        .iter()
        .any(|r| r.path() == "/fhir/Condition" && r.target.contains("snomed.info")));
    let before = mock.requests().len();
    for elsewhere in [
        "https://elsewhere.example/fhir/Patient".to_string(),
        "//elsewhere.example/fhir/Patient".to_string(),
        format!("{}x/Patient", mock.fhir_base()),
    ] {
        let err = api(&elsewhere).unwrap_err();
        assert_eq!(err.exit_code(), 2, "{err}");
        assert!(err.to_string().contains("only"), "{err}");
    }
    assert_eq!(mock.requests().len(), before);
}

#[test]
fn the_test_organization_never_signs_in_at_another_machine() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    // A base set by hand in the file, past `config set`'s own check.
    let path = ctx.store.path().unwrap();
    std::fs::write(
        &path,
        r#"{"test_fhir_base":"https://mock.example:8080/fhir","profiles":{"adult1":{}}}"#,
    )
    .unwrap();
    let err = commands::auth::run(
        &ctx,
        &AuthCmd::Login(LoginArgs {
            no_browser: true,
            ..login_args("test", None)
        }),
    )
    .unwrap_err();
    assert_eq!(err.exit_code(), 2, "{err}");
    assert!(!err.to_string().contains("synthetic-secret"));
    assert!(mock.requests().is_empty());
}

#[test]
fn one_unreadable_sign_in_does_not_hide_the_others() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    let mut state = settings(&ctx);
    let mut other = state.grants[0].clone();
    other.id = "pat-0002".into();
    other.label = Some("child1".into());
    ctx.vault
        .put(
            &entry(&other.account()),
            &pk_cli_secrets::Secret::new("{not json"),
        )
        .unwrap();
    state.upsert(other);
    save(&ctx, &state);
    let (_, status) = commands::auth::status_of(&ctx).unwrap();
    let rows = status["patients"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["label"], "adult1");
    assert_eq!(rows[0]["usable"], true);
    assert_eq!(rows[0]["renewable"], true);
    assert!(rows[0].get("error").is_none());
    assert_eq!(rows[1]["label"], "child1");
    assert_eq!(rows[1]["usable"], false);
    assert!(rows[1]["error"].as_str().is_some_and(|e| !e.is_empty()));
    assert!(!rows[1]["error"].as_str().unwrap().contains("pat-0002"));
    assert_eq!(
        rows[1]["remedy"],
        "run `modmed auth logout --profile adult1 --org test --patient child1`, then `modmed auth login --profile adult1 --org test --label child1`"
    );
    assert!(rows[0].get("remedy").is_none());
    assert_eq!(status["authenticated"], true);

    let list = commands::patients::list(&ctx, true, None).unwrap();
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["usable"], true);
    assert_eq!(items[1]["usable"], false);
    assert!(items[1]["error"].is_string());
    assert!(items[1]["refused"].is_null());
    assert_eq!(items[1]["remedy"], rows[1]["remedy"]);
    assert_eq!(items[0]["refused"], false);
    let test = commands::patients::list(&ctx, false, Some("TEST")).unwrap();
    assert_eq!(test["items"].as_array().unwrap().len(), 2);
    let other = commands::patients::list(&ctx, false, Some("example.mmi")).unwrap();
    assert_eq!(other["items"], serde_json::json!([]));
}

#[test]
fn logout_forget_removes_the_sign_ins_the_clients_and_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    commands::auth::run(
        &ctx,
        &AuthCmd::Logout(LogoutArgs {
            who: PatientArgs::default(),
            forget: false,
        }),
    )
    .unwrap();
    assert_eq!(ctx.vault.names().unwrap(), ["_client/test"]);
    commands::auth::run(
        &ctx,
        &AuthCmd::Logout(LogoutArgs {
            who: PatientArgs::default(),
            forget: true,
        }),
    )
    .unwrap();
    assert!(credentials::read_client(&ctx.vault, Environment::Test)
        .unwrap()
        .is_none());
    assert!(ctx.vault.names().unwrap().is_empty());
}

#[test]
fn a_sign_in_finishes_when_chrome_does_not_open_and_the_address_is_opened_by_hand() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let mut ctx = ctx(dir.path(), &mock);
    // The window fails; the patient opens the printed address themselves,
    // which the follower stands in for, and modmed is still listening.
    let opened = window();
    ctx.window = Box::new(move |profile: &ProfileName, url: &str| {
        opened(profile, url)?;
        Err(pk_cli_core::CliError::Other(
            "Chrome is not installed".into(),
        ))
    });
    login(&ctx, "adult1");
    assert_eq!(settings(&ctx).grants.len(), 1);
}

#[test]
fn a_refused_renewal_stops_the_export_and_keeps_the_last_complete_one() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    let out = dir.path().join("out");
    export(&ctx, &out, None);
    let complete = std::fs::read(out.join("records.ndjson")).unwrap();

    // The practice refuses the token and then the refresh token: the
    // sign-in is gone, which no retry fixes.
    mock.fail(Fault::unauthorized("/fhir/").times(10));
    mock.fail(
        Fault::new("POST", "/token", 400, r#"{"error":"invalid_grant"}"#)
            .header("Content-Type", "application/json")
            .times(10),
    );
    let failed = commands::records::export(&ctx, &export_args(Some(&out), None))
        .unwrap()
        .expect("the export ends on the refusal");
    assert_eq!(failed.exit_code(), 3, "{failed}");
    assert_eq!(
        failed.remedy(),
        Some("run `modmed auth login --profile adult1 --org test --label adult1`")
    );
    assert_eq!(std::fs::read(out.join("records.ndjson")).unwrap(), complete);
    let incomplete = std::fs::read_to_string(out.join("records.incomplete.ndjson")).unwrap();
    let trailer: serde_json::Value =
        serde_json::from_str(incomplete.lines().last().unwrap()).unwrap();
    assert_eq!(trailer["complete"], false);

    // The refusal is recorded, so the next use renews before it sends the
    // refused token again.
    let item = ctx.vault.get(&entry(ACCOUNT)).unwrap().unwrap();
    let session = pk_med_smart::decode_session(item.expose()).unwrap();
    assert!(session.refused_at.is_some());
}

#[test]
fn documents_list_and_download_the_patients_files() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    let range = pk_cli_documents::RangeArgs::default();
    let out = dir.path().join("docs");
    commands::documents::run(
        &ctx,
        &DocumentsCmd::Download {
            id: None,
            all: true,
            output: out.clone(),
            who: PatientArgs::default(),
            range: range.clone(),
        },
    )
    .unwrap();
    let files: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(files, ["2020-01-02_doc-0001.pdf"]);
    let bytes = std::fs::read(out.join(&files[0])).unwrap();
    assert_eq!(bytes, pk_med_fhir::test_util::PDF);
    // One by id, to a file.
    let one = dir.path().join("one.pdf");
    commands::documents::run(
        &ctx,
        &DocumentsCmd::Download {
            id: Some("doc-0001".into()),
            all: false,
            output: one.clone(),
            who: PatientArgs::default(),
            range: range.clone(),
        },
    )
    .unwrap();
    assert_eq!(std::fs::read(&one).unwrap(), pk_med_fhir::test_util::PDF);
    // An id the list does not have is not found.
    let err = commands::documents::run(
        &ctx,
        &DocumentsCmd::Download {
            id: Some("doc-9999".into()),
            all: false,
            output: dir.path().join("none.pdf"),
            who: PatientArgs::default(),
            range,
        },
    )
    .unwrap_err();
    assert_eq!(err.exit_code(), 4);
}

#[test]
fn a_document_whose_bytes_are_not_what_its_type_says_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start();
    let ctx = ctx(dir.path(), &mock);
    login(&ctx, "adult1");
    // "hello" in base64, served as the PDF.
    mock.fail(
        Fault::new(
            "GET",
            "/fhir/Binary/bin-0001",
            200,
            r#"{"resourceType":"Binary","id":"bin-0001","contentType":"application/pdf","data":"aGVsbG8="}"#,
        )
        .header("Content-Type", "application/fhir+json")
        .times(5),
    );
    let out = dir.path().join("docs");
    commands::documents::run(
        &ctx,
        &DocumentsCmd::Download {
            id: None,
            all: true,
            output: out.clone(),
            who: PatientArgs::default(),
            range: Default::default(),
        },
    )
    .unwrap();
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
}
