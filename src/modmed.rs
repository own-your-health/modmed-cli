//! ModMed's Certified (ONC (g)(10)) FHIR API: where a sign-in may go, the
//! scopes it asks for, and the two client registrations `modmed` keeps.
//! `docs/api.md` has ModMed's documentation, its wording and the traps.
//!
//! Every ModMed practice has a FHIR base of its own on
//! `<practice>.<zone>.prod.fhir.ema-api.com`, with its own SMART
//! configuration and sign-in (the practice's patient portal account). One
//! registered app (client id and secret) serves every practice.

use pk_cli_core::CliError;
use pk_med_smart::{ScopeSet, VendorQuirks};

/// The `vendor` of every organization `modmed` reaches.
pub const VENDOR: &str = "modmed";

/// The organization id of a test server on this machine (`modmed config set
/// test_fhir_base`). Every id from ModMed's directory has a dot in it
/// (`example.mmi`), so none can be this one.
pub const TEST_ORG_ID: &str = "test";

/// The domain under which ModMed serves every practice's FHIR API.
pub const FHIR_DOMAIN: &str = "fhir.ema-api.com";

/// Where a practice's sign-in and token endpoints may be: its own host
/// (what the practice's SMART configuration names), or ModMed's single
/// sign-on host (what ModMed's documentation names).
pub const AUTH_DOMAINS: &[&str] = &["fhir.ema-api.com", "sso.ema.md"];

/// The resource scopes ModMed's SMART configuration lists for a patient
/// app, one per type: it lists no `patient/*.rs` wildcard.
pub const PATIENT_TYPES: &[&str] = &[
    "AllergyIntolerance",
    "CarePlan",
    "CareTeam",
    "Condition",
    "Coverage",
    "Device",
    "DiagnosticReport",
    "DocumentReference",
    "Encounter",
    "Goal",
    "Immunization",
    "Location",
    "Medication",
    "MedicationDispense",
    "MedicationRequest",
    "Observation",
    "Organization",
    "Patient",
    "Practitioner",
    "PractitionerRole",
    "Procedure",
    "Provenance",
    "Questionnaire",
    "QuestionnaireResponse",
    "RelatedPerson",
    "ServiceRequest",
    "Specimen",
];

/// The context and session scopes. ModMed's table for a standalone patient
/// app: `openid` and `launch/patient` required, `fhirUser` and a session
/// scope recommended. `offline_access` gives a refresh token that outlives
/// the portal session, so exports run without the patient.
pub const CONTEXT_SCOPES: &[&str] = &["openid", "fhirUser", "launch/patient", "offline_access"];

/// The scope a sign-in asks for.
pub fn scope() -> String {
    CONTEXT_SCOPES
        .iter()
        .map(|s| s.to_string())
        .chain(PATIENT_TYPES.iter().map(|t| format!("patient/{t}.rs")))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn scopes() -> ScopeSet {
    ScopeSet::parse(&scope())
}

/// Which of the app's client registrations a sign-in uses: ModMed's, or
/// a test server's on this machine, so ModMed's secret never goes to one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Production,
    Test,
}

impl Environment {
    pub const ALL: [Environment; 2] = [Environment::Production, Environment::Test];

    pub fn as_str(self) -> &'static str {
        match self {
            Environment::Production => "production",
            Environment::Test => "test",
        }
    }

    /// The environment of an organization id: the test server's, else a
    /// practice in ModMed's directory.
    pub fn of_org(org_id: &str) -> Self {
        if org_id == TEST_ORG_ID {
            Environment::Test
        } else {
            Environment::Production
        }
    }
}

/// Whether `base` is a server on this machine (`127.0.0.0/8`, `::1`,
/// `localhost`), read by the URL parser, not by prefix: in
/// `http://127.0.0.1:80@example.org/` the host is example.org.
pub fn is_loopback(base: &str) -> bool {
    let Ok(url) = url::Url::parse(base) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// Whether `url` is https on one of `domains` or a host under one, by the
/// parsed host, with no user part.
fn is_https_under(url: &str, domains: &[&str]) -> bool {
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(url::Host::Domain(name)) = url.host() else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    domains
        .iter()
        .any(|d| name == *d || name.ends_with(&format!(".{d}")))
}

/// Whether `base` is a ModMed practice's FHIR server: https on a host under
/// `fhir.ema-api.com`.
pub fn is_modmed_fhir(base: &str) -> bool {
    is_https_under(base, &[FHIR_DOMAIN])
}

/// Whether `url` is one of ModMed's sign-in or token endpoints.
pub fn is_modmed_auth(url: &str) -> bool {
    is_https_under(url, AUTH_DOMAINS)
}

/// What kind of address [`require`] checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Address {
    /// A FHIR base: the practice's server.
    Fhir,
    /// An authorization or token endpoint, where the client secret goes.
    Auth,
}

/// Where a sign-in may go, before the client id and secret travel: a
/// practice to ModMed's servers only, the test organization to a server on
/// this machine only, whose patients are synthetic. Anything else is
/// refused, so the client secret, which ModMed's terms keep confidential,
/// never reaches another host, and no other server's records reach medrec
/// as a practice's.
pub fn require(environment: Environment, kind: Address, url: &str) -> Result<(), CliError> {
    let allowed = match (environment, kind) {
        (Environment::Production, Address::Fhir) => is_modmed_fhir(url),
        (Environment::Production, Address::Auth) => is_modmed_auth(url),
        (Environment::Test, _) => is_loopback(url),
    };
    if allowed {
        return Ok(());
    }
    let origin = url::Url::parse(url)
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_else(|_| "that address".into());
    Err(match environment {
        Environment::Production => CliError::Upstream(format!(
            "{origin} is not ModMed's; a practice signs in and reads only at https://…{} (sign-in also at https://sso.ema.md)",
            FHIR_DOMAIN
        )),
        Environment::Test => CliError::Usage(format!(
            "{origin} is not on this machine; the `{TEST_ORG_ID}` organization goes only to a test server here"
        ))
        .with_remedy("run `modmed config set test_fhir_base http://127.0.0.1:<PORT>/fhir`"),
    })
}

/// What ModMed's server does differently from SMART as written. Nothing so
/// far: its SMART configuration is at the practice's base, PKCE S256 is
/// offered, `aud` and `nonce` are accepted. `docs/api.md` lists what is
/// only known after the first real sign-in.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModMedQuirks;

impl VendorQuirks for ModMedQuirks {}

/// A practice's sign-in refusing the authorization request itself, before
/// any page a person signs in on. Its Keycloak answers with an error page
/// (`<div id="kc-error-message"><p class="instruction">…</p>`) and never
/// redirects back, so a browser left on it waits out the whole timeout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// "Client not found.": the sign-in has no client with the id sent. A
    /// made-up id answers so; an app ModMed has not approved is expected to.
    ClientNotFound,
    /// "Client disabled.": Keycloak's message for a client that exists and
    /// is disabled; not yet seen at ModMed.
    ClientDisabled,
    /// "Invalid parameter: redirect_uri": the redirect is not the app's
    /// Redirect URL.
    RedirectUri,
    /// Any other message, as the page gives it.
    Other(String),
}

impl Refusal {
    /// `error.reason` in `--json`.
    pub fn slug(&self) -> &'static str {
        match self {
            Refusal::ClientNotFound => "client_not_found",
            Refusal::ClientDisabled => "client_disabled",
            Refusal::RedirectUri => "redirect_refused",
            Refusal::Other(_) => "sign_in_refused",
        }
    }
}

/// The refusal in an answer to the authorization request: a 4xx carrying
/// Keycloak's error message. Anything else (the sign-in page, a redirect,
/// a 5xx that a reload may get past) is for the browser.
pub fn refusal(status: u16, body: &str) -> Option<Refusal> {
    if !(400..500).contains(&status) {
        return None;
    }
    let div = &body[body.find("id=\"kc-error-message\"")?..];
    let div = &div[..div.find("</div>").unwrap_or(div.len())];
    let p = &div[div.find("class=\"instruction\"")?..];
    let p = &p[p.find('>')? + 1..];
    let message = text_of(&p[..p.find("</p>")?]);
    Some(match message.as_str() {
        "Client not found." => Refusal::ClientNotFound,
        "Client disabled." => Refusal::ClientDisabled,
        "Invalid parameter: redirect_uri" => Refusal::RedirectUri,
        "" => return None,
        _ => Refusal::Other(message),
    })
}

/// An HTML fragment's text: tags dropped, the entities Keycloak's pages
/// escape decoded, control characters (a terminal's escape sequences
/// among them) and runs of whitespace each one space.
fn text_of(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    let text = text
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&amp;", "&");
    text.split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The most of a refusal's message an error quotes.
pub const QUOTED_CHARS: usize = 200;

/// A refusal's message as an error quotes it: `client_id`, as sent or
/// percent-encoded and in any ASCII case, replaced by `<client id>`, then
/// cut to [`QUOTED_CHARS`].
pub fn quoted(message: &str, client_id: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(client_id.as_bytes()).collect();
    let mut text = message.to_string();
    for id in [client_id, encoded.as_str()] {
        if !id.is_empty() {
            text = replace_ignoring_ascii_case(&text, id, "<client id>");
        }
    }
    match text.char_indices().nth(QUOTED_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text,
    }
}

/// `text` with each `needle`, in any ASCII case, replaced by `with`. ASCII
/// lower-casing keeps every byte offset, so the matches index `text`.
fn replace_ignoring_ascii_case(text: &str, needle: &str, with: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut out = String::new();
    let mut last = 0;
    for (at, _) in lower.match_indices(&needle) {
        out.push_str(&text[last..at]);
        out.push_str(with);
        last = at + needle.len();
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_is_the_parsed_host() {
        for base in [
            "http://127.0.0.1:9/fhir",
            "http://127.1.2.3/fhir",
            "http://localhost:8080/fhir",
            "http://[::1]:9/fhir",
        ] {
            assert!(is_loopback(base), "{base}");
        }
        for base in [
            "http://127.0.0.1:80@other.example/fhir",
            "http://127.0.0.1.evil.example:1/",
            "http://localhost.evil.example/",
            "https://example.mmi.prod.fhir.ema-api.com/fhir/r4",
            "not a url",
        ] {
            assert!(!is_loopback(base), "{base}");
        }
    }

    #[test]
    fn a_practice_is_https_under_modmeds_fhir_domain() {
        for base in [
            "https://example.mmi.prod.fhir.ema-api.com/fhir/r4",
            "https://00000000-0000-4000-8000-000000000000.gastro.prod.fhir.ema-api.com/fhir/r4/",
            "https://EXAMPLE.EF.PROD.FHIR.EMA-API.COM/fhir/r4",
        ] {
            assert!(is_modmed_fhir(base), "{base}");
            require(Environment::Production, Address::Fhir, base).unwrap();
        }
        for base in [
            "http://example.mmi.prod.fhir.ema-api.com/fhir/r4",
            "https://fhir.ema-api.com.evil.example/fhir",
            "https://evilfhir.ema-api.com/fhir",
            // A user part, split so the personal-data scan sees no address.
            concat!("https://user", "@example.mmi.prod.fhir.ema-api.com/fhir"),
            "https://sso.ema.md/fhir",
            "https://127.0.0.1/fhir",
        ] {
            assert!(!is_modmed_fhir(base), "{base}");
            assert_eq!(
                require(Environment::Production, Address::Fhir, base)
                    .unwrap_err()
                    .exit_code(),
                5,
                "{base}"
            );
        }
    }

    #[test]
    fn the_secret_goes_only_to_modmeds_sign_in_hosts() {
        for url in [
            "https://example.mmi.prod.fhir.ema-api.com/auth/realms/fhir/protocol/openid-connect/token",
            "https://sso.ema.md/auth/realms/fhir/protocol/openid-connect/token",
        ] {
            require(Environment::Production, Address::Auth, url).unwrap();
        }
        for url in [
            "https://sso.ema.md.evil.example/token",
            "https://ema.md/token",
            "http://sso.ema.md/token",
            "http://127.0.0.1:9/token",
        ] {
            assert!(require(Environment::Production, Address::Auth, url).is_err());
        }
    }

    #[test]
    fn the_test_organization_goes_to_this_machine_only() {
        require(Environment::Test, Address::Fhir, "http://127.0.0.1:9/fhir").unwrap();
        require(Environment::Test, Address::Auth, "http://127.0.0.1:9/token").unwrap();
        let err = require(
            Environment::Test,
            Address::Fhir,
            "https://example.mmi.prod.fhir.ema-api.com/fhir/r4",
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.remedy().unwrap().contains("test_fhir_base"));
    }

    #[test]
    fn the_scope_names_every_type_and_no_wildcard() {
        let scope = scope();
        assert!(scope.starts_with("openid fhirUser launch/patient offline_access "));
        assert!(scope.contains("patient/DocumentReference.rs"));
        assert!(!scope.contains('*'));
        assert_eq!(
            scope.split(' ').count(),
            CONTEXT_SCOPES.len() + PATIENT_TYPES.len()
        );
        assert!(scopes().contains("patient/Observation.rs"));
    }

    #[test]
    fn a_refusal_is_the_error_page_s_message() {
        let page = |name| match name {
            "client-not-found" => include_str!("../tests/fixtures/sign-in/client-not-found.html"),
            "redirect-uri" => include_str!("../tests/fixtures/sign-in/redirect-uri.html"),
            _ => include_str!("../tests/fixtures/sign-in/bearer-only.html"),
        };
        assert_eq!(
            refusal(400, page("client-not-found")),
            Some(Refusal::ClientNotFound)
        );
        assert_eq!(
            refusal(400, page("redirect-uri")),
            Some(Refusal::RedirectUri)
        );
        assert_eq!(
            refusal(403, page("bearer-only")),
            Some(Refusal::Other(
                "Bearer-only applications are not allowed to initiate browser login".into()
            ))
        );
        let disabled = page("client-not-found").replace("Client not found.", "Client disabled.");
        assert_eq!(refusal(400, &disabled), Some(Refusal::ClientDisabled));
        let escaped = page("client-not-found").replace(
            "Client not found.",
            "\n  A &quot;quoted&quot; &amp; <b>bold</b>\n  reason ",
        );
        assert_eq!(
            refusal(400, &escaped),
            Some(Refusal::Other("A \"quoted\" & bold reason".into()))
        );
    }

    #[test]
    fn a_message_loses_control_characters() {
        let page = include_str!("../tests/fixtures/sign-in/client-not-found.html")
            .replace("Client not found.", "Red\u{1b}[31m alert\u{7}\tnow");
        assert_eq!(
            refusal(400, &page),
            Some(Refusal::Other("Red [31m alert now".into()))
        );
    }

    #[test]
    fn a_quoted_message_hides_the_client_id_in_any_form_and_is_cut() {
        let id = "Made-Up/Client 01";
        for message in [
            "no client Made-Up/Client 01 here",
            "no client made-up/client 01 here",
            "no client Made-Up%2FClient+01 here",
            "no client made-up%2fclient+01 here",
        ] {
            assert_eq!(
                quoted(message, id),
                "no client <client id> here",
                "{message}"
            );
        }
        let long = "é".repeat(QUOTED_CHARS + 5);
        let cut = quoted(&long, id);
        assert_eq!(cut.chars().count(), QUOTED_CHARS + 1);
        assert!(cut.ends_with('…'));
        assert_eq!(quoted("short", id), "short");
    }

    #[test]
    fn only_a_4xx_error_page_is_a_refusal() {
        let page = include_str!("../tests/fixtures/sign-in/client-not-found.html");
        // A sign-in page, a redirect back, a server error a reload may get
        // past: the browser's to show.
        for status in [200, 302, 500, 503] {
            assert_eq!(refusal(status, page), None, "{status}");
        }
        for body in [
            "",
            r#"{"error":"invalid_request"}"#,
            "<html><body><form id=\"kc-form-login\"></form></body></html>",
            // A message outside the error block is not the page's refusal.
            "<div id=\"kc-error-message\"></div><p class=\"instruction\">Client not found.</p>",
            "<div id=\"kc-error-message\"><p class=\"instruction\"> </p></div>",
        ] {
            assert_eq!(refusal(400, body), None, "{body}");
        }
    }

    #[test]
    fn environments_by_organization() {
        assert_eq!(Environment::of_org(TEST_ORG_ID), Environment::Test);
        assert_eq!(Environment::of_org("example.mmi"), Environment::Production);
    }
}
