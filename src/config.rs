//! Non-secret settings (`~/.config/modmed/config.json`): the redirect, a test
//! server's FHIR base, another directory, and the profiles, one per patient
//! portal account, each with the patients it signed in. The client id and
//! secret ModMed issues are credentials under ModMed's Certified API Terms
//! of Use, so they live in modmed's vault (`modmed auth set-credential`),
//! never here; so do the sign-ins' tokens.

use pk_cli_config::profile::Profiles;
use pk_cli_core::CliError;
use serde::{Deserialize, Serialize};

use crate::modmed;
use crate::session::Context;

/// The loopback port the sign-in's redirect comes back to. ModMed matches
/// the redirect URI registered for the app, so the port is fixed.
pub const DEFAULT_REDIRECT_PORT: u16 = 7470;
pub const REDIRECT_PATH: &str = "/callback";
pub const REDIRECT_HOST: &str = "127.0.0.1";

pub const KEYS: &str = "redirect_port, redirect_page, test_fhir_base, directory_url";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_port: Option<u16>,
    /// An https page registered with ModMed as the redirect, which forwards
    /// the browser to the loopback port with the query unchanged, for when
    /// ModMed accepts no `http://127.0.0.1` redirect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_page: Option<String>,
    /// The FHIR base of a test server on this machine: the organization
    /// `test`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_fhir_base: Option<String>,
    /// One directory to read instead of ModMed's two: a test server's on
    /// this machine, or ModMed's at another address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory_url: Option<String>,
    /// One per patient portal account (`adult1`), each with its
    /// [`ProfileSettings`].
    #[serde(default, skip_serializing_if = "Profiles::is_empty")]
    pub profiles: Profiles,
}

/// A profile's settings: the patients its portal account signed in, one per
/// (organization, patient). Their tokens are in the vault, as
/// `<profile>/<organization>/<patient>`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ProfileSettings {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<Context>,
}

impl ProfileSettings {
    /// Insert or replace the grant for the same (organization, patient).
    pub fn upsert(&mut self, context: Context) {
        self.remove(&context.organization, &context.id);
        self.grants.push(context);
    }

    pub fn remove(&mut self, organization: &str, id: &str) {
        self.grants
            .retain(|c| !(c.organization == organization && c.id == id));
    }
}

impl Config {
    pub fn redirect_port(&self) -> u16 {
        self.redirect_port.unwrap_or(DEFAULT_REDIRECT_PORT)
    }

    /// `http://127.0.0.1:<port>/callback`, the loopback the answer comes
    /// back to.
    pub fn loopback_uri(&self) -> String {
        format!(
            "http://{REDIRECT_HOST}:{}{REDIRECT_PATH}",
            self.redirect_port()
        )
    }

    /// The redirect ModMed sees: the registered page, else the loopback.
    pub fn redirect_uri(&self) -> String {
        self.redirect_page
            .clone()
            .unwrap_or_else(|| self.loopback_uri())
    }

    /// The directories `orgs` reads, as `(name, url)`.
    pub fn directories(&self) -> Vec<(String, String)> {
        match &self.directory_url {
            Some(url) => vec![("configured".to_string(), url.clone())],
            None => crate::orgs::DIRECTORIES
                .iter()
                .map(|(n, u)| (n.to_string(), u.to_string()))
                .collect(),
        }
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<(), CliError> {
        match key {
            "redirect_port" => {
                let port: u16 = value.parse().ok().filter(|p| *p >= 1024).ok_or_else(|| {
                    CliError::Usage(format!("`{value}` is not a port from 1024 to 65535"))
                })?;
                self.redirect_port = Some(port);
            }
            "redirect_page" => {
                let url = url::Url::parse(value)
                    .map_err(|_| CliError::Usage(format!("`{value}` is not a URL")))?;
                if url.scheme() != "https"
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.fragment().is_some()
                {
                    return Err(CliError::Usage(
                        "redirect_page is an https page with a host, no user and no fragment"
                            .into(),
                    ));
                }
                self.redirect_page = Some(value.to_string());
            }
            "test_fhir_base" => {
                pk_cli_core::transport::require_secure_url(value)?;
                if !modmed::is_loopback(value) {
                    return Err(CliError::Usage(
                        "test_fhir_base is a server on this machine (http://127.0.0.1:<PORT>/…); practices come from ModMed's directory".into(),
                    ));
                }
                self.test_fhir_base = Some(pk_med_fhir::normalize_base(value));
            }
            "directory_url" => {
                pk_cli_core::transport::require_secure_url(value)?;
                if !crate::orgs::allowed_directory(value) {
                    return Err(CliError::Usage(
                        "directory_url is ModMed's (https under fhir.ema-api.com) or a test server on this machine".into(),
                    ));
                }
                self.directory_url = Some(value.to_string());
            }
            other => return Err(unknown(other)),
        }
        Ok(())
    }

    pub fn unset(&mut self, key: &str) -> Result<(), CliError> {
        match key {
            "redirect_port" => self.redirect_port = None,
            "redirect_page" => self.redirect_page = None,
            "test_fhir_base" => self.test_fhir_base = None,
            "directory_url" => self.directory_url = None,
            other => return Err(unknown(other)),
        }
        Ok(())
    }
}

fn unknown(key: &str) -> CliError {
    CliError::Usage(format!("unknown config key `{key}` (known: {KEYS})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = Config::default();
        assert_eq!(c.redirect_uri(), "http://127.0.0.1:7470/callback");
        assert_eq!(c.directories().len(), 2);
        assert!(c.test_fhir_base.is_none());
    }

    #[test]
    fn keys_set_and_unset() {
        let mut c = Config::default();
        c.set("redirect_port", "8123").unwrap();
        assert_eq!(c.redirect_uri(), "http://127.0.0.1:8123/callback");
        assert!(c.set("redirect_port", "80").is_err());
        c.set(
            "redirect_page",
            "https://example.github.io/modmed-cli/callback",
        )
        .unwrap();
        assert_eq!(
            c.redirect_uri(),
            "https://example.github.io/modmed-cli/callback"
        );
        assert_eq!(c.loopback_uri(), "http://127.0.0.1:8123/callback");
        for bad in [
            "http://example.github.io/cb",
            "https://u@cb.example/cb",
            "https://example.github.io/cb#x",
            "nope",
        ] {
            assert!(c.set("redirect_page", bad).is_err(), "{bad}");
        }
        assert!(c
            .set(
                "test_fhir_base",
                "https://example.mmi.prod.fhir.ema-api.com/fhir/r4"
            )
            .is_err());
        assert!(c.set("test_fhir_base", "http://example.com/fhir").is_err());
        c.set("test_fhir_base", "http://127.0.0.1:9/fhir/").unwrap();
        assert_eq!(c.test_fhir_base.as_deref(), Some("http://127.0.0.1:9/fhir"));
        c.set("directory_url", "http://127.0.0.1:9/fhir/Endpoint")
            .unwrap();
        assert_eq!(c.directories()[0].1, "http://127.0.0.1:9/fhir/Endpoint");
        assert!(c
            .set("directory_url", "https://example.com/Endpoint")
            .is_err());
        for key in [
            "redirect_port",
            "redirect_page",
            "test_fhir_base",
            "directory_url",
        ] {
            c.unset(key).unwrap();
        }
        assert_eq!(serde_json::to_string(&c).unwrap(), "{}");
        assert_eq!(c.set("nope", "x").unwrap_err().exit_code(), 2);
        assert_eq!(c.unset("nope").unwrap_err().exit_code(), 2);
    }
}
