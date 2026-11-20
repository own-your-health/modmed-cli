//! Command implementations. `main` builds a [`Ctx`] and dispatches; the
//! tests build one over a vault with an in-memory key, a scratch config
//! directory and lock, and a stand-in browser.

pub mod api;
pub mod auth;
pub mod config;
pub mod documents;
pub mod orgs;
pub mod patients;
pub mod profiles;
pub mod records;
pub mod registration;

use std::any::Any;
use std::path::PathBuf;
use std::sync::Arc;

use pk_cli_auth::refresh_lock::RefreshLock;
use pk_cli_auth::vault::Vault;
use pk_cli_config::profile::{self, ProfileName};
use pk_cli_config::ConfigStore;
use pk_cli_core::{CliError, CommonArgs};
use pk_cli_http::BlockingTransport;
use pk_cli_secrets::CredentialStore;
use pk_med_smart::ProfileSessions;

use crate::cli::PatientArgs;
use crate::config::{Config, ProfileSettings};
use crate::credentials::{self, ClientCredentials};
use crate::modmed::{Environment, TEST_ORG_ID};
use crate::orgs::{Freshness, Org, OrgIndex};
use crate::session::{Context, Live};

pub const BIN: &str = "modmed";
pub const REPO: &str = "piekstra/modmed-cli";

/// Names the keychain service a run uses instead of `piekstra.modmed`. For the
/// test suite and for dogfooding: a service that holds nothing reads as
/// signed out without a keychain prompt. With it, the vault's entries live
/// beside the config file (`vault.<service>/`), so a test's entries never
/// meet the real vault's key.
pub const KEYCHAIN_SERVICE_ENV: &str = "MODMED_KEYCHAIN_SERVICE";

/// The environment's fallback for `--profile`.
pub const PROFILE_ENV: &str = "MODMED_PROFILE";

/// Names the directory index's cache directory instead of
/// `${XDG_CACHE_HOME:-~/.cache}/modmed` (tests).
pub const CACHE_DIR_ENV: &str = "MODMED_CACHE_DIR";

/// Opens the sign-in address in the profile's own browser; the window stays
/// open while the returned value lives.
pub type OpenWindow = Box<dyn Fn(&ProfileName, &str) -> Result<Box<dyn Any>, CliError>>;

/// Removes a profile's kept browser profile; whether there was one.
pub type ForgetBrowser = Box<dyn Fn(&ProfileName) -> Result<bool, CliError>>;

pub struct Ctx {
    pub common: CommonArgs,
    pub store: ConfigStore,
    /// `--profile`, before it is checked against the config.
    pub profile: Option<String>,
    pub vault: Arc<Vault>,
    pub lock: RefreshLock,
    /// Where the directory index is kept.
    pub cache: PathBuf,
    pub window: OpenWindow,
    pub forget_browser: ForgetBrowser,
    /// Listen on any loopback port, not the configured one (tests, whose
    /// mock server redirects wherever it is asked to).
    pub any_port: bool,
}

impl Ctx {
    /// The binary's context: modmed's vault (or one under
    /// `MODMED_KEYCHAIN_SERVICE`), the family's refresh lock,
    /// `~/.config/modmed`, `~/.cache/modmed`, Chrome in the profile's own
    /// browser profile.
    pub fn for_binary(
        common: CommonArgs,
        config: Option<PathBuf>,
        profile: Option<String>,
    ) -> Result<Self, CliError> {
        let store = ConfigStore::new(BIN).with_override(config);
        let vault = match keychain_override() {
            Some(service) => {
                let dir = store
                    .path()?
                    .parent()
                    .map(|d| d.join(format!("vault.{service}")))
                    .ok_or_else(|| CliError::Other("the config path has no directory".into()))?;
                Vault::new(CredentialStore::new(service), dir)
            }
            None => Vault::for_binary(BIN)?,
        };
        let cache = match std::env::var_os(CACHE_DIR_ENV).filter(|v| !v.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => crate::orgs::cache_dir()?,
        };
        Ok(Ctx {
            common,
            store,
            profile,
            vault: Arc::new(vault),
            lock: RefreshLock::for_binary(BIN)?,
            cache,
            window: Box::new(|profile, url| {
                pk_med_smart::SignInWindow::open_at(BIN, profile, url, &[])
                    .map(|w| Box::new(w) as Box<dyn Any>)
            }),
            forget_browser: Box::new(|profile| {
                pk_cli_browser::forget_named_profile(BIN, profile.as_str()).map_err(CliError::from)
            }),
            any_port: false,
        })
    }

    pub fn json(&self) -> bool {
        self.common.json
    }

    pub fn config(&self) -> Result<Config, CliError> {
        self.store.load()
    }

    /// Write the config. Its profiles' grants carry each patient's FHIR
    /// patient id, and `ConfigStore::save` (cli-common v0.33.1) writes it
    /// owner-only and whole, so a crash mid-write loses no profile.
    pub fn save_config(&self, config: &Config) -> Result<(), CliError> {
        self.store.save(config)
    }

    /// The profile this command runs as: `--profile`, else `MODMED_PROFILE`. It
    /// must be configured; there is no default.
    pub fn profile(&self, config: &Config) -> Result<ProfileName, CliError> {
        profile::resolve(self.profile.as_deref(), PROFILE_ENV, &config.profiles, BIN)
    }

    pub fn settings(
        &self,
        config: &Config,
        profile: &ProfileName,
    ) -> Result<ProfileSettings, CliError> {
        config.profiles.settings(profile)
    }

    /// Replace a profile's settings in the config file.
    pub fn save_settings(
        &self,
        profile: &ProfileName,
        settings: &ProfileSettings,
    ) -> Result<(), CliError> {
        let mut config = self.config()?;
        config.profiles.set_settings(profile, settings)?;
        self.save_config(&config)
    }

    pub fn sessions(&self, profile: &ProfileName) -> ProfileSessions<'_> {
        ProfileSessions::new(&self.vault, profile.clone())
    }

    /// The profile's patient `who` names (the only one, when one is held).
    /// `--org` matches a grant's organization id or name, any case, so no
    /// directory download is needed to pick one.
    pub fn signed_in(
        &self,
        config: &Config,
        profile: &ProfileName,
        who: &PatientArgs,
    ) -> Result<Context, CliError> {
        let settings = self.settings(config, profile)?;
        let org = who.org.as_deref().map(|q| grant_org(&settings.grants, q));
        crate::session::pick(&settings.grants, who.patient.as_deref(), org, profile).cloned()
    }

    /// The directory index: the cached copy, downloaded first when there is
    /// none, it is a week old, or `force`. A failed download with an older
    /// copy warns and answers from it.
    pub fn org_index(&self, config: &Config, force: bool) -> Result<OrgIndex, CliError> {
        let (index, freshness) = self.org_index_downloaded(config, force)?;
        if let Freshness::Stale(e) = freshness {
            self.note(format!(
                "warning: ModMed's directory could not be downloaded ({}); using the copy from {}",
                e.message(),
                rfc3339(index.fetched_at.max(0) as u64)
            ));
        }
        Ok(index)
    }

    /// The directory index and how it was had: from the cache, downloaded
    /// now, or an older copy after a failed download (which the caller
    /// reports).
    pub fn org_index_downloaded(
        &self,
        config: &Config,
        force: bool,
    ) -> Result<(OrgIndex, Freshness), CliError> {
        let t = transport()?;
        let quiet = self.common.quiet;
        let note = move |line: String| {
            if !quiet {
                eprintln!("{line}");
            }
        };
        let (index, freshness) = crate::orgs::ensure(
            &t,
            &self.cache,
            &config.directories(),
            now_secs(),
            force,
            &note,
        )?;
        if matches!(freshness, Freshness::Refreshed) {
            self.note(format!(
                "downloaded ModMed's directories: {} practices",
                index.orgs.len()
            ));
        }
        Ok((index, freshness))
    }

    /// `--org` for a sign-in: `test` for the test server (`config set
    /// test_fhir_base`), else a practice in ModMed's directory.
    pub fn resolve_org(&self, config: &Config, query: &str) -> Result<Org, CliError> {
        let q = query.trim();
        if q.eq_ignore_ascii_case(TEST_ORG_ID) {
            return match &config.test_fhir_base {
                Some(base) => Ok(Org::test(base)),
                None => Err(CliError::Usage(
                    "the `test` organization is a test server on this machine, and none is configured".into(),
                )
                .with_remedy(format!(
                    "run `{BIN} config set test_fhir_base http://127.0.0.1:<PORT>/fhir`"
                ))),
            };
        }
        if q.is_empty() {
            return Err(CliError::Usage("--org is empty".into()));
        }
        let index = self.org_index(config, false)?;
        crate::orgs::resolve(&index.orgs, q).cloned()
    }

    /// The client credentials for an environment, or the auth error that
    /// says how to store them.
    pub fn client(&self, environment: Environment) -> Result<ClientCredentials, CliError> {
        credentials::read_client(&self.vault, environment)?.ok_or_else(|| {
            CliError::Auth(format!(
                "no {} client id and secret are stored",
                environment.as_str()
            ))
            .with_remedy(match environment {
                Environment::Production => format!(
                    "register your own app with ModMed (`{BIN} registration` prints the form's values; {}), then run `{BIN} auth set-credential`",
                    crate::registration::DOC
                ),
                Environment::Test => format!("run `{BIN} auth set-credential --test`"),
            })
        })
    }

    /// A grant's sign-in, renewed when its token has lapsed.
    pub fn live(
        &self,
        profile: &ProfileName,
        context: &Context,
    ) -> Result<Live<BlockingTransport>, CliError> {
        let client = self.client(context.environment())?;
        Live::start(
            transport()?,
            self.vault.clone(),
            profile.clone(),
            self.lock.clone(),
            client,
            context,
        )
        .map_err(|e| sign_in_again(e, profile, context))
    }

    pub fn note(&self, msg: impl AsRef<str>) {
        if !self.common.quiet {
            eprintln!("{}", msg.as_ref());
        }
    }
}

fn keychain_override() -> Option<String> {
    std::env::var(KEYCHAIN_SERVICE_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// A sign-in error's remedy as modmed's own command for this patient: a
/// refused or missing sign-in is `auth login`, a stored one that does not
/// read is `auth logout` then `auth login`, and one a newer modmed wrote is
/// `self-update`. Other errors keep their remedy.
pub fn sign_in_again(e: CliError, profile: &ProfileName, context: &Context) -> CliError {
    let org = context.organization.as_str();
    let label = context
        .label
        .as_deref()
        .map(|l| format!(" --label {l}"))
        .unwrap_or_default();
    let login = format!("`{BIN} auth login --profile {profile} --org {org}{label}`");
    match e.remedy() {
        Some(pk_med_smart::remedy::UPDATE) => {
            return e.with_remedy(format!("run `{BIN} self-update`"));
        }
        Some(pk_med_smart::remedy::CLEAR_AND_SIGN_IN_AGAIN) => {
            // The patient's id stays out of the message: a label, or the
            // placeholder `patients list` fills.
            let (who, found) = match context.label.as_deref() {
                Some(label) => (label.to_string(), String::new()),
                None => (
                    "<WHO>".to_string(),
                    format!(" (`{BIN} patients list --profile {profile}` shows who)"),
                ),
            };
            return e.with_remedy(format!(
                "run `{BIN} auth logout --profile {profile} --org {org} --patient {who}`{found}, then {login}"
            ));
        }
        _ => {}
    }
    if e.exit_code() != 3 {
        return e;
    }
    e.with_remedy(format!("run {login}"))
}

/// An error as one line for stderr, its remedy after it, for a command
/// whose stdout already carries its output.
pub fn report(e: &CliError) {
    eprintln!("{}", report_line(e));
}

/// cli-common's own spelling, `message — remedy`, as every other error
/// prints, so a caller (medrec) reads the remedy the same way from both.
fn report_line(e: &CliError) -> String {
    format!("error: {e}")
}

/// The transport for ModMed's FHIR, OAuth and directory endpoints: no
/// redirects, the family's 60 s timeout.
pub fn transport() -> Result<BlockingTransport, CliError> {
    BlockingTransport::new()
}

/// Unix seconds now.
pub fn now_secs() -> i64 {
    crate::session::now() as i64
}

/// The organization id a `--org` names among a profile's grants: a grant's
/// id or name in any case, else the text as given, which then matches none.
pub fn grant_org<'a>(grants: &'a [Context], query: &'a str) -> &'a str {
    let q = query.trim();
    grants
        .iter()
        .find(|c| {
            c.organization.eq_ignore_ascii_case(q)
                || c.organization_name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(q))
        })
        .map(|c| c.organization.as_str())
        .unwrap_or(q)
}

/// `value` as JSON, for a DTO this crate built.
pub fn to_json<T: serde::Serialize>(value: &T) -> Result<serde_json::Value, CliError> {
    serde_json::to_value(value).map_err(|e| CliError::Other(format!("serializing output: {e}")))
}

/// RFC 3339 for Unix seconds.
pub fn rfc3339(secs: u64) -> String {
    pk_cli_core::dates::fmt_rfc3339(secs as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reported_error_puts_its_remedy_after_an_em_dash() {
        let e = CliError::Auth("the sign-in was refused".into())
            .with_remedy("run `modmed auth login --org test --label adult1`");
        assert_eq!(
            report_line(&e),
            "error: the sign-in was refused \u{2014} run `modmed auth login --org test --label adult1`"
        );
        let plain = CliError::Upstream("the practice answered 503".into());
        assert_eq!(
            report_line(&plain),
            "error: network/upstream error: the practice answered 503"
        );
    }

    #[test]
    fn an_org_matches_a_grant_by_id_or_name() {
        let grant = |org: &str, name: Option<&str>| Context {
            id: "pat-0001".into(),
            organization: org.into(),
            organization_name: name.map(Into::into),
            label: None,
            relationship: None,
            fhir_user: None,
            signed_in_at: "2020-01-01T00:00:00Z".into(),
        };
        let grants = [
            grant("example.mmi", Some("Example Dermatology")),
            grant("test", None),
        ];
        assert_eq!(grant_org(&grants, "EXAMPLE.MMI"), "example.mmi");
        assert_eq!(grant_org(&grants, "example dermatology"), "example.mmi");
        assert_eq!(grant_org(&grants, "test"), "test");
        assert_eq!(grant_org(&grants, "other.ef"), "other.ef");
    }
}
