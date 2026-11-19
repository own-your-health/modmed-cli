//! The patient sign-ins a profile holds: pk-med-smart's `SmartSession` in
//! modmed's vault, one entry per (organization, patient) named
//! `<profile>/<organization>/<patient>` (`ProfileSessions`), and a grant for
//! each in the profile's settings (`Context`), so `patients list` reads no
//! keychain item.

use std::sync::{Arc, Mutex};

use pk_cli_auth::oauth::{ClientAuth, OAuthClient};
use pk_cli_auth::refresh_lock::RefreshLock;
use pk_cli_auth::vault::Vault;
use pk_cli_config::profile::ProfileName;
use pk_cli_core::transport::Transport;
use pk_cli_core::CliError;
use pk_cli_secrets::Secret;
use pk_med_records::dto::{kind, relationship};
use pk_med_records::PatientContext;
use pk_med_smart::{ProfileSessions, Renewer, SessionStore, SmartSession};
use serde::{Deserialize, Serialize};

use crate::credentials::ClientCredentials;
use crate::modmed::{self, Address, Environment, ModMedQuirks};

/// One grant in a profile's settings: everything about a sign-in but its
/// tokens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Context {
    /// The patient's FHIR `Patient.id` at the practice (`--patient`).
    pub id: String,
    /// modmed's organization id (`example.mmi`, or `test`).
    pub organization: String,
    /// The practice's name when signed in, for messages and listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relationship: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fhir_user: Option<String>,
    /// RFC 3339.
    pub signed_in_at: String,
}

impl Context {
    pub fn from_session(
        session: &SmartSession,
        organization_name: Option<String>,
        label: Option<String>,
        signed_in_at: String,
    ) -> Self {
        let id = session.patient.clone().unwrap_or_default();
        Context {
            relationship: relationship_of(session.fhir_user.as_deref(), &id),
            id,
            organization: session.organization.clone(),
            organization_name,
            label,
            fhir_user: session.fhir_user.clone(),
            signed_in_at,
        }
    }

    pub fn account(&self) -> String {
        pk_med_smart::account(&self.organization, &self.id)
    }

    pub fn environment(&self) -> Environment {
        Environment::of_org(&self.organization)
    }

    /// The practice as a stream header names it, at the sign-in's FHIR base.
    pub fn organization(&self, fhir_base: &str) -> pk_med_records::Organization {
        if self.environment() == Environment::Test {
            return crate::orgs::Org::test(fhir_base).organization();
        }
        pk_med_records::Organization {
            id: self.organization.clone(),
            name: self
                .organization_name
                .clone()
                .unwrap_or_else(|| self.organization.clone()),
            fhir_base: pk_med_fhir::normalize_base(fhir_base),
            vendor: Some(modmed::VENDOR.to_string()),
            location: None,
            aliases: Vec::new(),
        }
    }

    /// The label, else the FHIR id.
    pub fn display(&self) -> String {
        self.label.clone().unwrap_or_else(|| self.id.clone())
    }

    /// The `patient-list/v1` item and the stream header's `patient`. The
    /// name is never read from the record.
    pub fn patient_context(&self) -> PatientContext {
        PatientContext {
            id: self.id.clone(),
            organization: self.organization.clone(),
            label: self.label.clone(),
            relationship: self.relationship.clone(),
            kind: Some(kind::PATIENT.to_string()),
            member_role: None,
            fhir_user: self.fhir_user.clone(),
            name: None,
            session: Some(self.account()),
        }
    }
}

/// `self` when the signed-in user is the patient, `proxy` when it is someone
/// else (a parent acting for a child); unknown when ModMed does not say.
fn relationship_of(fhir_user: Option<&str>, patient: &str) -> Option<String> {
    let user = fhir_user?;
    let own =
        user == format!("Patient/{patient}") || user.ends_with(&format!("/Patient/{patient}"));
    Some(
        if own {
            relationship::SELF
        } else {
            relationship::PROXY
        }
        .to_string(),
    )
}

/// Resolve `--patient` (and `--org`, an organization id) against a
/// profile's grants: with none given and one held, that one.
pub fn pick<'a>(
    contexts: &'a [Context],
    patient: Option<&str>,
    org: Option<&str>,
    profile: &ProfileName,
) -> Result<&'a Context, CliError> {
    let narrowed: Vec<Context> = contexts
        .iter()
        .filter(|c| org.is_none_or(|o| c.organization == o))
        .cloned()
        .collect();
    if narrowed.is_empty() {
        // The remedy signs in where the caller asked. `--patient` is not
        // echoed: it may be a patient's FHIR id rather than a label.
        let at = org.unwrap_or("<ORG>");
        return Err(CliError::Auth(match org {
            Some(o) => format!("profile {profile} has no patient signed in at {o}"),
            None => format!("profile {profile} has no patient signed in"),
        })
        .with_remedy(format!(
            "run `modmed auth login --profile {profile} --org {at} --label <WHO>`"
        )));
    }
    let chosen = match patient {
        None if narrowed.len() == 1 => narrowed[0].clone(),
        None => {
            return Err(CliError::Usage(format!(
                "{} patients are signed in; name one with --patient ({})",
                narrowed.len(),
                narrowed
                    .iter()
                    .map(|c| format!("{} at {}", c.display(), c.organization))
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
        }
        Some(q) => pk_cli_core::resolve::pick(
            &narrowed,
            q,
            |c| vec![c.id.clone()],
            |c| c.label.as_deref().unwrap_or(&c.id),
            "signed-in patient",
        )?
        .clone(),
    };
    contexts
        .iter()
        .find(|c| c.organization == chosen.organization && c.id == chosen.id)
        .ok_or_else(|| CliError::Other("the chosen patient vanished from the list".into()))
}

/// How modmed authenticates to ModMed's token endpoint: the confidential
/// client's id and secret in the form body (`client_secret_post`, the only
/// method ModMed's SMART configuration lists).
pub fn client_auth(client: &ClientCredentials) -> ClientAuth {
    ClientAuth::SecretPost {
        client_id: client.client_id.clone(),
        secret: client.secret(),
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A sign-in in use: its session renewed when lapsed, and renewed again
/// when the practice refuses its token mid-run. Owns what renewal needs, so
/// the FHIR client can call it from its `bearer_with` source.
#[derive(Clone)]
pub struct Live<T> {
    pub account: String,
    vault: Arc<Vault>,
    profile: ProfileName,
    lock: RefreshLock,
    oauth: OAuthClient<T>,
    client: ClientCredentials,
    current: Arc<Mutex<SmartSession>>,
}

impl<T: Transport + Clone + Send + Sync + 'static> Live<T> {
    /// The context's session, renewed first when its token has lapsed.
    pub fn start(
        transport: T,
        vault: Arc<Vault>,
        profile: ProfileName,
        lock: RefreshLock,
        client: ClientCredentials,
        context: &Context,
    ) -> Result<Self, CliError> {
        let account = context.account();
        let sessions = ProfileSessions::new(&vault, profile.clone());
        // Where the stored sign-in points is checked before a renewal sends
        // the client id and secret to its token endpoint.
        if let Some(stored) = sessions.load(&account)? {
            let environment = context.environment();
            modmed::require(environment, Address::Fhir, &stored.fhir_base)?;
            modmed::require(environment, Address::Auth, &stored.token_endpoint)?;
        }
        let oauth = OAuthClient::new(transport, crate::commands::BIN, env!("CARGO_PKG_VERSION"));
        let auth = client_auth(&client);
        let session =
            Renewer::new(&sessions, &lock, &oauth, &auth, &ModMedQuirks).fresh(&account, now())?;
        drop(sessions);
        Ok(Live {
            account,
            vault,
            profile,
            lock,
            oauth,
            client,
            current: Arc::new(Mutex::new(session)),
        })
    }

    pub fn session(&self) -> SmartSession {
        self.current.lock().expect("the session lock").clone()
    }

    pub fn fhir_base(&self) -> String {
        self.session().fhir_base
    }

    /// The bearer for the next request: the current token, or after the
    /// practice refused `refused`, a renewed one.
    pub fn bearer(&self, refused: Option<&Secret>) -> Result<Secret, CliError> {
        let Some(refused) = refused else {
            return Ok(self.session().bearer());
        };
        let auth = client_auth(&self.client);
        let sessions = ProfileSessions::new(&self.vault, self.profile.clone());
        let renewed = Renewer::new(&sessions, &self.lock, &self.oauth, &auth, &ModMedQuirks)
            .after_refusal(&self.account, refused.expose(), now())?;
        let bearer = renewed.bearer();
        *self.current.lock().expect("the session lock") = renewed;
        Ok(bearer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(id: &str, label: Option<&str>, org: &str) -> Context {
        Context {
            id: id.into(),
            organization: org.into(),
            organization_name: None,
            label: label.map(Into::into),
            relationship: None,
            fhir_user: None,
            signed_in_at: "2020-01-01T00:00:00Z".into(),
        }
    }

    fn adult1() -> ProfileName {
        ProfileName::parse("adult1").unwrap()
    }

    #[test]
    fn the_remedy_signs_in_where_the_caller_asked() {
        for who in ["adult1", "pat-0001"] {
            let err = pick(&[], Some(who), Some("test"), &adult1()).unwrap_err();
            assert_eq!(err.exit_code(), 3);
            assert!(err.to_string().contains("at test"), "{err}");
            assert!(!err.to_string().contains("pat-0001"), "{err}");
            assert_eq!(
                err.remedy(),
                Some("run `modmed auth login --profile adult1 --org test --label <WHO>`")
            );
        }
        let err = pick(&[], None, None, &adult1()).unwrap_err();
        assert_eq!(
            err.remedy(),
            Some("run `modmed auth login --profile adult1 --org <ORG> --label <WHO>`")
        );
    }

    #[test]
    fn pick_takes_the_only_one_or_asks() {
        let p = adult1();
        let one = [ctx("pat-0001", Some("adult1"), "test")];
        assert_eq!(pick(&one, None, None, &p).unwrap().id, "pat-0001");
        assert_eq!(pick(&[], None, None, &p).unwrap_err().exit_code(), 3);
        let two = [
            ctx("pat-0001", Some("adult1"), "test"),
            ctx("pat-0002", Some("child1"), "example.mmi"),
        ];
        let err = pick(&two, None, None, &p).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("child1 at example.mmi"), "{err}");
        assert_eq!(pick(&two, Some("child1"), None, &p).unwrap().id, "pat-0002");
        assert_eq!(
            pick(&two, Some("pat-0001"), None, &p).unwrap().id,
            "pat-0001"
        );
        assert_eq!(
            pick(&two, Some("nobody"), None, &p)
                .unwrap_err()
                .exit_code(),
            4
        );
        assert_eq!(
            pick(&two, None, Some("example.mmi"), &p).unwrap().id,
            "pat-0002"
        );
        assert_eq!(
            pick(&two, None, Some("other.mmi"), &p)
                .unwrap_err()
                .exit_code(),
            3
        );
    }

    #[test]
    fn relationship_comes_from_fhir_user_only() {
        assert_eq!(relationship_of(None, "p1"), None);
        assert_eq!(
            relationship_of(Some("Patient/p1"), "p1").as_deref(),
            Some("self")
        );
        assert_eq!(
            relationship_of(Some("https://x.example/fhir/Patient/p1"), "p1").as_deref(),
            Some("self")
        );
        assert_eq!(
            relationship_of(Some("RelatedPerson/r1"), "p1").as_deref(),
            Some("proxy")
        );
    }

    #[test]
    fn a_context_is_a_patient_with_its_keychain_account() {
        let c = ctx("pat-0001", Some("adult1"), "test").patient_context();
        assert_eq!(c.kind.as_deref(), Some("patient"));
        assert_eq!(c.session.as_deref(), Some("smart/test/pat-0001"));
        assert!(c.name.is_none());
    }

    #[test]
    fn the_secret_goes_in_the_body() {
        let client = ClientCredentials::new("client-one", "secret-one").unwrap();
        assert!(matches!(
            client_auth(&client),
            ClientAuth::SecretPost { .. }
        ));
    }
}
