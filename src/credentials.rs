//! The client id and secret ModMed issues for this household's app: one
//! entry per environment in modmed's vault (`_client/production`, and
//! `_client/test` for a test server on this machine), beside the profiles'
//! sign-ins, so the whole CLI reads one keychain item. A leading `_` is a
//! name no profile has, so no profile lists or removes them. ModMed's
//! Certified API Terms of Use (2.4) say credentials may not be embedded in
//! open source projects, so neither is ever printed, put in a plain file,
//! committed or taken from argv.

use pk_cli_auth::vault::Vault;
use pk_cli_core::CliError;
use pk_cli_secrets::Secret;
use serde::{Deserialize, Serialize};

use crate::modmed::Environment;

/// The vault entry for an environment's client registration.
pub fn client_entry(environment: Environment) -> String {
    format!("_client/{}", environment.as_str())
}

/// The app's client id and secret for one environment. No `Debug`: it holds
/// the secret.
#[derive(Clone, Serialize, Deserialize)]
pub struct ClientCredentials {
    pub client_id: String,
    pub client_secret: String,
}

impl ClientCredentials {
    /// Check a pasted id and secret without quoting either in an error.
    pub fn new(client_id: &str, client_secret: &str) -> Result<Self, CliError> {
        let id = client_id.trim();
        // RFC 7617: HTTP Basic's user-id cannot hold a colon.
        if id.is_empty()
            || id.len() > 200
            || id.contains(':')
            || !id.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(CliError::Usage(
                "the client id is the printable string ModMed's vendor dashboard shows for the app, with no spaces or colons".into(),
            ));
        }
        let secret = client_secret.trim();
        if secret.is_empty() || secret.len() > 1000 || secret.chars().any(char::is_control) {
            return Err(CliError::Usage(
                "the client secret is the single line ModMed's vendor dashboard shows for the app"
                    .into(),
            ));
        }
        Ok(ClientCredentials {
            client_id: id.to_string(),
            client_secret: secret.to_string(),
        })
    }

    /// Split `<id>:<secret>` or `<id>` and `<secret>` on two lines, as
    /// `--stdin` and `--from-env` take them.
    pub fn parse_pair(text: &Secret) -> Result<Self, CliError> {
        let text = text.expose().trim();
        let split = text
            .split_once('\n')
            .or_else(|| text.split_once(':'))
            .ok_or_else(|| {
                CliError::Usage(
                    "give the client id and secret as `<id>:<secret>`, or on two lines".into(),
                )
            })?;
        Self::new(split.0, split.1)
    }

    pub fn secret(&self) -> Secret {
        Secret::new(self.client_secret.clone())
    }
}

pub fn read_client(
    vault: &Vault,
    environment: Environment,
) -> Result<Option<ClientCredentials>, CliError> {
    vault.get_json(&client_entry(environment))
}

pub fn store_client(
    vault: &Vault,
    environment: Environment,
    client: &ClientCredentials,
) -> Result<(), CliError> {
    vault.put_json(&client_entry(environment), client)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_reads_in_either_form() {
        let a = ClientCredentials::parse_pair(&Secret::new("client-one:secret-one\n")).unwrap();
        assert_eq!(
            (a.client_id.as_str(), a.client_secret.as_str()),
            ("client-one", "secret-one")
        );
        let b = ClientCredentials::parse_pair(&Secret::new("client-two\nsec:ret\n")).unwrap();
        assert_eq!(
            (b.client_id.as_str(), b.client_secret.as_str()),
            ("client-two", "sec:ret")
        );
    }

    #[test]
    fn a_bad_pair_is_a_usage_error_that_quotes_nothing() {
        for bad in [
            "only-one-part",
            "has space:secret",
            ":secret",
            "id:",
            "id\n",
        ] {
            let err = ClientCredentials::parse_pair(&Secret::new(bad))
                .err()
                .unwrap();
            assert_eq!(err.exit_code(), 2, "{bad}");
            let message = err.to_string();
            assert!(
                !message.contains("secret-") && !message.contains("has space"),
                "{message}"
            );
        }
    }

    fn vault(dir: &std::path::Path) -> Vault {
        Vault::new(pk_cli_auth::vault::MemoryKeys::new(), dir.join("vault"))
    }

    #[test]
    fn the_client_round_trips_through_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        let vault = vault(dir.path());
        assert!(read_client(&vault, Environment::Test).unwrap().is_none());
        let c = ClientCredentials::new("client-one", "secret-one").unwrap();
        store_client(&vault, Environment::Test, &c).unwrap();
        assert_eq!(vault.names().unwrap(), ["_client/test"]);
        let back = read_client(&vault, Environment::Test).unwrap().unwrap();
        assert_eq!(back.client_id, "client-one");
        assert!(read_client(&vault, Environment::Production)
            .unwrap()
            .is_none());
    }

    #[test]
    fn an_unreadable_entry_is_an_error_not_absent() {
        let dir = tempfile::tempdir().unwrap();
        let vault = vault(dir.path());
        vault
            .put(
                &client_entry(Environment::Production),
                &Secret::new("{not json"),
            )
            .unwrap();
        assert!(read_client(&vault, Environment::Production).is_err());
    }
}
