//! `modmed auth set-credential|login|status|logout`.

use std::io::IsTerminal;
use std::time::Duration;

use pk_cli_auth::oauth::OAuthClient;
use pk_cli_auth::{AuthLogin, AuthMethod, AuthStatus};
use pk_cli_core::{block_on, output, CliError};
use pk_cli_secrets::Secret;
use pk_med_fhir::FhirClient;
use pk_med_smart::{begin, discover, LaunchRequest, SessionStore, SmartApp, SmartSession};
use serde_json::{json, Value};

use super::{rfc3339, transport, Ctx, BIN};
use crate::cli::{AuthCmd, LoginArgs, LogoutArgs, SetCredentialArgs, LOGIN_TIMEOUT};
use crate::credentials::{self, client_entry, ClientCredentials};
use crate::modmed::{self, Address, Environment, ModMedQuirks};
use crate::orgs::Org;
use crate::session::{client_auth, now, Context};

pub fn run(ctx: &Ctx, cmd: &AuthCmd) -> Result<(), CliError> {
    match cmd {
        AuthCmd::SetCredential(args) => set_credential(ctx, args),
        AuthCmd::Login(args) => login(ctx, args),
        AuthCmd::Status => status(ctx),
        AuthCmd::Logout(args) => logout(ctx, args),
    }
}

fn set_credential(ctx: &Ctx, args: &SetCredentialArgs) -> Result<(), CliError> {
    let done = store_credential(ctx, args)?;
    output::emit(ctx.json(), "client-credential", done.clone(), |_| {
        println!(
            "Stored the {} client id and secret in modmed's vault.",
            done["environment"].as_str().unwrap_or_default()
        )
    });
    Ok(())
}

fn store_credential(ctx: &Ctx, args: &SetCredentialArgs) -> Result<Value, CliError> {
    if args.source.stdin && args.source.from_env.is_some() {
        return Err(CliError::Usage(
            "pass exactly one of --stdin or --from-env".into(),
        ));
    }
    let environment = environment_of(args);
    let interactive =
        !args.source.stdin && args.source.from_env.is_none() && std::io::stdin().is_terminal();
    // Input that comes from a flag or a pipe is checked before the Keychain
    // is opened; a prompt waits until the refusal below, so nobody types a
    // secret only to have it turned away.
    let piped = if interactive {
        None
    } else {
        Some(read_piped(args)?)
    };
    store_pair(ctx, environment, args.overwrite, piped)
}

/// The registration `--test` picks: a test server's, else ModMed's.
fn environment_of(args: &SetCredentialArgs) -> Environment {
    if args.test {
        Environment::Test
    } else {
        Environment::Production
    }
}

/// Stores `piped`, or the pair the prompts ask for when there is none,
/// unless a client is stored for the environment and `overwrite` is not
/// given. `client-credential/v1`'s payload: `environment`, `stored`, and
/// the `vault_entry` it is stored as.
pub fn store_pair(
    ctx: &Ctx,
    environment: Environment,
    overwrite: bool,
    piped: Option<ClientCredentials>,
) -> Result<Value, CliError> {
    if !overwrite && credentials::read_client(&ctx.vault, environment)?.is_some() {
        return Err(CliError::ConfirmationRequired(format!(
            "a {} client is already stored",
            environment.as_str()
        ))
        .with_remedy("pass --overwrite to replace it"));
    }
    let client = match piped {
        Some(client) => client,
        None => prompt_client()?,
    };
    credentials::store_client(&ctx.vault, environment, &client)?;
    Ok(json!({
        "environment": environment.as_str(),
        "stored": true,
        "vault_entry": client_entry(environment),
    }))
}

/// The id and secret at two prompts that do not echo.
fn prompt_client() -> Result<ClientCredentials, CliError> {
    let id = Secret::prompt("ModMed client id (not echoed)")?;
    let secret = Secret::prompt("ModMed client secret (not echoed)")?;
    ClientCredentials::new(id.expose(), secret.expose())
}

/// The pair from `--stdin`, `--from-env`, or stdin when it is not a terminal.
fn read_piped(args: &SetCredentialArgs) -> Result<ClientCredentials, CliError> {
    let pair = if args.source.stdin || args.source.from_env.is_some() {
        args.source.read(None)?
    } else {
        pk_cli_secrets::read_stdin()?
    };
    ClientCredentials::parse_pair(&pair)
}

/// A label names a person in this household (`adult1`), never read from a
/// record: short, printable, no spaces.
fn check_label(label: &str) -> Result<(), CliError> {
    let ok = !label.is_empty()
        && label.len() <= 40
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if ok {
        Ok(())
    } else {
        Err(CliError::Usage(
            "a label is up to 40 letters, digits, `-`, `_` or `.` (`adult1`, `child1`)".into(),
        ))
    }
}

/// What a sign-in that ends without one says to do: the same command again,
/// for the same profile, organization and label.
fn login_again(
    profile: &pk_cli_config::profile::ProfileName,
    org: &Org,
    args: &LoginArgs,
) -> String {
    let mut command = format!("{BIN} auth login --profile {profile} --org {}", org.id);
    if let Some(label) = &args.label {
        command.push_str(&format!(" --label {label}"));
    }
    if args.no_browser {
        command.push_str(" --no-browser");
    }
    if args.timeout != LOGIN_TIMEOUT {
        command.push_str(&format!(" --timeout {}", args.timeout));
    }
    format!("run `{command}` again")
}

/// An error keeps the remedy it came with, and gets `again` otherwise.
fn or_again(e: CliError, again: &str) -> CliError {
    if e.remedy().is_some() {
        e
    } else {
        e.with_remedy(again)
    }
}

fn login(ctx: &Ctx, args: &LoginArgs) -> Result<(), CliError> {
    if let Some(label) = &args.label {
        check_label(label)?;
    }
    if args.timeout == 0 || args.timeout > 60 {
        return Err(CliError::Usage("--timeout is from 1 to 60 minutes".into()));
    }
    if args.org.trim().is_empty() {
        return Err(
            CliError::Usage("--org is empty".into()).with_remedy(format!(
                "find the practice with `{BIN} orgs search <WORDS>`"
            )),
        );
    }
    let config = ctx.config()?;
    let profile = ctx.profile(&config)?;
    let org = ctx.resolve_org(&config, &args.org)?;
    let environment = Environment::of_org(&org.id);
    modmed::require(environment, Address::Fhir, &org.fhir_base)?;
    let client = ctx.client(environment)?;
    let t = transport()?;
    let smart = block_on(discover(
        &t,
        &org.fhir_base,
        &ModMedQuirks,
        BIN,
        env!("CARGO_PKG_VERSION"),
    ))?;
    // The secret goes to the token endpoint the practice names; it too must
    // be ModMed's, or for the test organization this machine's.
    modmed::require(environment, Address::Auth, &smart.authorization_endpoint)?;
    modmed::require(environment, Address::Auth, &smart.token_endpoint)?;
    ctx.lock.before_signin()?;
    let mut app = SmartApp::new(
        BIN,
        client_auth(&client),
        if ctx.any_port && config.redirect_page.is_none() {
            Vec::new()
        } else {
            vec![config.redirect_port()]
        },
    );
    if let Some(page) = &config.redirect_page {
        app = app.redirect_uri(page.clone());
    }
    let pending = begin(
        &app,
        LaunchRequest {
            config: &smart,
            fhir_base: &org.fhir_base,
            aud: Some(org.aud()),
            scopes: modmed::scopes(),
            quirks: &ModMedQuirks,
        },
    )?;
    // Chrome in the profile's own browser profile, so one portal account's
    // cookies never sign in another. The authorize step is the practice's
    // own sign-in, so no page opens before it.
    let window = if args.no_browser {
        None
    } else {
        match (ctx.window)(&profile, pending.url()) {
            Ok(window) => Some(window),
            Err(e) => {
                eprintln!("error: Chrome did not open: {e}");
                eprintln!(
                    "warning: any other browser shares its cookies across modmed's profiles: sign \
                     out of the practice's patient portal there first, or it may sign in another \
                     profile's account"
                );
                None
            }
        }
    };
    if window.is_some() {
        ctx.note(format!(
            "Signing in to {} in Chrome (profile {profile}); waiting for it to finish.",
            org.name
        ));
    } else {
        ctx.note(format!(
            "Open this address in a browser and sign in to the patient portal of {}:",
            org.name
        ));
        eprintln!("{}", pending.url());
    }
    let again = login_again(&profile, &org, args);
    let code = pending
        .wait_for_code(Duration::from_secs(args.timeout * 60), |reason| {
            ctx.note(format!(
                "{reason}. Still waiting; press Ctrl-C to start again."
            ))
        })
        .map_err(|e| or_again(e, &again))?;
    // Chrome quits, writing the cookies it received to the profile's
    // browser profile.
    drop(window);
    let oauth = OAuthClient::new(t.clone(), BIN, env!("CARGO_PKG_VERSION"));
    let session = block_on(pending.exchange(&oauth, &client_auth(&client), &code, &org.id, now()))
        .map_err(|e| or_again(e, &again))?;
    if session.patient.as_deref().is_none_or(str::is_empty) {
        return Err(CliError::Upstream(
            "the practice's token answer named no patient (the launch/patient context); nothing was stored"
                .into(),
        ));
    }
    if !session.renewable() {
        ctx.note(
            "warning: the practice granted no refresh token, so this sign-in lasts only as long as its token; `auth status` shows when it ends",
        );
    }
    // The grant first: it is the index `logout` removes vault entries by, so
    // a session never exists without one. A grant whose session failed to
    // store reads as signed out, and is removed if it is new.
    let context = Context::from_session(
        &session,
        (!org.is_test()).then(|| org.name.clone()),
        args.label.clone(),
        rfc3339(now()),
    );
    let mut settings = ctx.settings(&ctx.config()?, &profile)?;
    let existed = settings
        .grants
        .iter()
        .any(|c| c.organization == context.organization && c.id == context.id);
    settings.upsert(context.clone());
    ctx.save_settings(&profile, &settings)?;
    let sessions = ctx.sessions(&profile);
    if let Err(e) = ctx.lock.store_signin(|| sessions.save(&session)) {
        if !existed {
            settings.remove(&context.organization, &context.id);
            let _ = ctx.save_settings(&profile, &settings);
        }
        return Err(e);
    }
    let verified = verify(&t, &session, &context.id);
    if !verified {
        ctx.note(format!(
            "Signed in, but reading the patient's Patient record failed; `{BIN} auth status --profile {profile}` shows the sign-in."
        ));
    }
    let mut v = AuthLogin::stored(verified).to_json();
    v["profile"] = profile.as_str().into();
    v["organization"] = serde_json::to_value(org.organization()).unwrap_or_default();
    v["patient"] = serde_json::to_value(context.patient_context()).unwrap_or_default();
    v["renewable"] = session.renewable().into();
    output::emit(ctx.json(), "auth-login", v, |_| {
        println!(
            "Signed in patient {} at {} ({}) in profile {profile}.",
            context.display(),
            org.name,
            org.id
        )
    });
    Ok(())
}

/// One read of the patient's own Patient record, kept nowhere: whether the
/// new sign-in works.
fn verify(t: &pk_cli_http::BlockingTransport, session: &SmartSession, patient: &str) -> bool {
    FhirClient::new(t, &session.fhir_base, BIN, env!("CARGO_PKG_VERSION"))
        .map(|c| c.bearer(session.bearer()))
        .and_then(|c| block_on(c.read("Patient", patient)))
        .is_ok()
}

/// `auth-status/v1` with `profile`, `clients` and `patients` rows. An entry
/// that cannot be read is its row's `error`, not the command's failure.
pub fn status_of(ctx: &Ctx) -> Result<(AuthStatus, Value), CliError> {
    let config = ctx.config()?;
    let profile = ctx.profile(&config)?;
    let settings = ctx.settings(&config, &profile)?;
    let sessions = ctx.sessions(&profile);
    let mut clients = Vec::new();
    let mut any_client = false;
    for environment in Environment::ALL {
        // The vault reads its key only when the entry exists, so a
        // logged-out status raises no prompt.
        let mut row = json!({ "environment": environment.as_str(), "stored": false });
        match credentials::read_client(&ctx.vault, environment) {
            Ok(found) => row["stored"] = found.is_some().into(),
            Err(e) => row["error"] = e.message().into(),
        }
        any_client |= row["stored"] == true;
        clients.push(row);
    }
    let mut rows = Vec::new();
    let mut any_stored = false;
    let mut any_usable = false;
    for c in &settings.grants {
        let mut row = serde_json::to_value(c.patient_context()).unwrap_or_default();
        let session = match sessions.load(&c.account()) {
            Ok(session) => session,
            Err(e) => {
                let e = super::sign_in_again(e, &profile, c);
                row["error"] = e.message().into();
                if let Some(remedy) = e.remedy() {
                    row["remedy"] = remedy.into();
                }
                None
            }
        };
        let usable = session.as_ref().is_some_and(|s| {
            (s.refused_at.is_none() && !s.tokens.is_expired(now(), 0)) || s.renewable()
        });
        any_stored |= session.is_some();
        any_usable |= usable;
        row["credential_in_keychain"] = session.is_some().into();
        row["usable"] = usable.into();
        row["renewable"] = session.as_ref().is_some_and(SmartSession::renewable).into();
        if let Some(at) = session.as_ref().and_then(|s| s.tokens.expires_at_rfc3339()) {
            row["token_expires_at"] = at.into();
        }
        rows.push(row);
    }
    let mut status = AuthStatus::new(true, any_usable, AuthMethod::BrowserSession);
    status.credential_in_keychain = Some(any_stored || any_client);
    status.session_valid = Some(any_usable);
    let mut v = status.to_json();
    v["profile"] = profile.as_str().into();
    v["clients"] = Value::Array(clients);
    v["patients"] = Value::Array(rows);
    Ok((status, v))
}

fn status(ctx: &Ctx) -> Result<(), CliError> {
    let (status, v) = status_of(ctx)?;
    let clients = v["clients"].as_array().cloned().unwrap_or_default();
    let rows = v["patients"].as_array().cloned().unwrap_or_default();
    if ctx.json() {
        output::json(&v);
    } else {
        println!(
            "Profile:       {}",
            v["profile"].as_str().unwrap_or_default()
        );
        status.render();
        for c in &clients {
            println!(
                "Client ({}): {}",
                c["environment"].as_str().unwrap_or_default(),
                if c["stored"] == true {
                    "stored".to_string()
                } else if let Some(e) = c["error"].as_str() {
                    format!("unreadable ({e})")
                } else if c["environment"] == "test" {
                    "not stored (`modmed auth set-credential --test`, test servers only)"
                        .to_string()
                } else {
                    "not stored (`modmed auth set-credential`)".to_string()
                }
            );
        }
        if rows.is_empty() {
            println!(
                "Patients:      none signed in (`modmed auth login --profile {} --org <ORG> --label <WHO>`)",
                v["profile"].as_str().unwrap_or_default()
            );
        } else {
            println!();
            let view: Vec<Value> = rows
                .iter()
                .map(|r| {
                    json!({
                        "patient": r["label"].as_str().unwrap_or(r["id"].as_str().unwrap_or_default()),
                        "organization": r["organization"],
                        "relationship": r["relationship"],
                        "usable": r["usable"],
                        "renewable": r["renewable"],
                        "token_expires": r["token_expires_at"],
                        "error": r["error"],
                        "remedy": r["remedy"],
                    })
                })
                .collect();
            output::table(&view);
        }
    }
    Ok(())
}

fn logout(ctx: &Ctx, args: &LogoutArgs) -> Result<(), CliError> {
    let done = signed_out(ctx, args)?;
    output::emit(ctx.json(), "auth-logout", done.clone(), |v| {
        let patients = v["signed_out"].as_array().map_or(0, Vec::len);
        let profile = v["profile"].as_str().unwrap_or_default();
        let kept: Vec<&str> = v["shared_kept_for"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let forgot = if !args.forget {
            String::new()
        } else if kept.is_empty() {
            "; removed the client credentials".into()
        } else {
            format!(
                "; kept the client credentials, which profile(s) {} still use",
                kept.join(", ")
            )
        };
        println!("Signed out {patients} patient(s) of profile {profile}{forgot}.");
    });
    Ok(())
}

/// Signs the profile's chosen patients out. With `--forget`, and only when
/// no other profile holds a sign-in, also removes what every profile
/// shares: the client pairs, and the vault's key once the vault is empty.
///
/// `auth-logout/v1`'s payload: `profile`; `removed`, the vault entries
/// deleted; `signed_out`, the patients' labels; `shared_kept_for`, the
/// other profiles whose sign-ins kept the shared items; `vault_cleared`.
fn signed_out(ctx: &Ctx, args: &LogoutArgs) -> Result<Value, CliError> {
    let config = ctx.config()?;
    let profile = ctx.profile(&config)?;
    let mut settings = ctx.settings(&config, &profile)?;
    let org = args
        .who
        .org
        .as_deref()
        .map(|q| super::grant_org(&settings.grants, q).to_string());
    let org = org.as_deref();
    let chosen: Vec<Context> = match &args.who.patient {
        Some(q) => vec![crate::session::pick(&settings.grants, Some(q), org, &profile)?.clone()],
        None => settings
            .grants
            .iter()
            .filter(|c| org.is_none_or(|o| c.organization == o))
            .cloned()
            .collect(),
    };
    if let (Some(o), true) = (org, chosen.is_empty()) {
        return Err(CliError::NotFound(format!(
            "no patient of profile {profile} is signed in at `{o}`"
        ))
        .with_remedy(format!(
            "`{BIN} patients list --profile {profile}` shows who is, and where"
        )));
    }
    let sessions = ctx.sessions(&profile);
    let mut removed = Vec::new();
    for c in &chosen {
        let account = c.account();
        if ctx.lock.forget(|| sessions.delete(&account))? {
            removed.push(sessions.entry_name(&account)?);
        }
        settings.remove(&c.organization, &c.id);
    }
    ctx.save_settings(&profile, &settings)?;
    let mut done = json!({
        "profile": profile.as_str(),
        "removed": [],
        "signed_out": chosen.iter().map(Context::display).collect::<Vec<_>>(),
        "shared_kept_for": [],
        "vault_cleared": false,
    });
    if args.forget {
        let others = profiles_holding_sign_ins(ctx, &profile)?;
        if others.is_empty() {
            for environment in Environment::ALL {
                let entry = client_entry(environment);
                if ctx.vault.delete(&entry)? {
                    removed.push(entry);
                }
            }
            // With nothing left in the vault, its key item goes too, so modmed
            // leaves nothing in the Keychain.
            let left = ctx.vault.list()?;
            if left.names.is_empty() && left.unreadable.is_empty() {
                ctx.vault.clear()?;
                done["vault_cleared"] = true.into();
            }
        } else {
            done["shared_kept_for"] = others.into();
        }
    }
    done["removed"] = removed.into();
    Ok(done)
}

/// The other profiles that still hold a sign-in: a grant in the config, or
/// an entry in the vault (a leading `_` is no profile's).
fn profiles_holding_sign_ins(
    ctx: &Ctx,
    profile: &pk_cli_config::profile::ProfileName,
) -> Result<Vec<String>, CliError> {
    let config = ctx.config()?;
    let mut others = std::collections::BTreeSet::new();
    for p in config.profiles.list().profiles {
        let name = pk_cli_config::profile::ProfileName::parse(&p.name)?;
        if &name != profile && !ctx.settings(&config, &name)?.grants.is_empty() {
            others.insert(p.name);
        }
    }
    for entry in ctx.vault.names()? {
        if let Some((owner, _)) = entry.split_once('/') {
            if !owner.starts_with('_') && owner != profile.as_str() {
                others.insert(owner.to_string());
            }
        }
    }
    Ok(others.into_iter().collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::cli::PatientArgs;
    use crate::config::Config;
    use pk_cli_auth::refresh_lock::RefreshLock;
    use pk_cli_auth::vault::{MemoryKeys, Vault};
    use pk_cli_config::profile::ProfileName;
    use pk_cli_config::ConfigStore;
    use std::sync::Arc;

    /// A context over a scratch directory and an in-memory key, with the
    /// profile `adult1` configured and selected.
    pub(crate) fn ctx(dir: &std::path::Path) -> Ctx {
        let store = ConfigStore::new("modmed").with_override(Some(dir.join("config.json")));
        let mut config = Config::default();
        config.profiles.add(ProfileName::parse("adult1").unwrap());
        store.save(&config).unwrap();
        Ctx {
            common: Default::default(),
            store,
            profile: Some("adult1".into()),
            vault: Arc::new(Vault::new(MemoryKeys::new(), dir.join("vault"))),
            lock: RefreshLock::at(dir.join("refresh.lock")),
            cache: dir.join("cache"),
            window: Box::new(|_, _| Err(CliError::Other("no browser in unit tests".into()))),
            forget_browser: Box::new(|_| Ok(false)),
            any_port: true,
        }
    }

    #[test]
    fn set_credential_refuses_to_replace_without_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let pair = |id: &str, secret: &str| Some(ClientCredentials::new(id, secret).unwrap());
        store_pair(
            &ctx,
            Environment::Production,
            false,
            pair("client-one", "secret-one"),
        )
        .unwrap();
        let err = store_pair(
            &ctx,
            Environment::Production,
            false,
            pair("client-two", "secret-two"),
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 6);
        store_pair(
            &ctx,
            Environment::Production,
            true,
            pair("client-two", "secret-two"),
        )
        .unwrap();
        let stored = credentials::read_client(&ctx.vault, Environment::Production)
            .unwrap()
            .unwrap();
        assert_eq!(stored.client_id, "client-two");
    }

    fn grant(id: &str, label: &str) -> Context {
        Context {
            id: id.into(),
            organization: crate::modmed::TEST_ORG_ID.into(),
            organization_name: None,
            label: Some(label.into()),
            relationship: None,
            fhir_user: None,
            signed_in_at: "2020-01-01T00:00:00Z".into(),
        }
    }

    fn forget() -> LogoutArgs {
        LogoutArgs {
            who: PatientArgs::default(),
            forget: true,
        }
    }

    fn keys(v: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        keys
    }

    fn source(stdin: bool, from_env: Option<&str>, test: bool) -> SetCredentialArgs {
        SetCredentialArgs {
            source: pk_cli_secrets::SecretSourceArgs {
                stdin,
                from_env: from_env.map(Into::into),
            },
            test,
            overwrite: false,
        }
    }

    #[test]
    fn test_picks_the_test_registration() {
        assert_eq!(environment_of(&source(true, None, true)), Environment::Test);
        assert_eq!(
            environment_of(&source(true, None, false)),
            Environment::Production
        );
    }

    #[test]
    fn stdin_and_from_env_together_are_refused_before_any_read() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        // The variable is never set: the refusal comes before it is read.
        let err = store_credential(&ctx, &source(true, Some("MODMED_TEST_NEVER_SET"), false))
            .unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(ctx.vault.names().unwrap().is_empty());
    }

    #[test]
    fn set_credential_names_its_environment_and_vault_entry() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let pair = ClientCredentials::new("client-one", "secret-one").unwrap();
        assert_eq!(
            store_pair(&ctx, Environment::Test, false, Some(pair)).unwrap(),
            json!({"environment": "test", "stored": true, "vault_entry": "_client/test"})
        );
    }

    #[test]
    fn logout_forget_keeps_what_profiles_share_while_another_holds_a_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        credentials::store_client(
            &ctx.vault,
            Environment::Test,
            &ClientCredentials::new("client-one", "secret-one").unwrap(),
        )
        .unwrap();
        let adult2 = ProfileName::parse("adult2").unwrap();
        let mut config = ctx.config().unwrap();
        config.profiles.add(adult2.clone());
        ctx.save_config(&config).unwrap();
        let mut settings = crate::config::ProfileSettings::default();
        settings.upsert(grant("pat-0002", "adult2"));
        ctx.save_settings(&adult2, &settings).unwrap();

        let done = signed_out(&ctx, &forget()).unwrap();
        assert_eq!(
            keys(&done),
            [
                "profile",
                "removed",
                "shared_kept_for",
                "signed_out",
                "vault_cleared"
            ]
        );
        assert_eq!(done["shared_kept_for"], json!(["adult2"]));
        assert_eq!(done["removed"], json!([]));
        assert_eq!(done["vault_cleared"], false);
        assert!(credentials::read_client(&ctx.vault, Environment::Test)
            .unwrap()
            .is_some());

        // adult2 signed out too: now the shared items go.
        let mut adult2_ctx = ctx;
        adult2_ctx.profile = Some("adult2".into());
        let done = signed_out(&adult2_ctx, &forget()).unwrap();
        assert_eq!(done["shared_kept_for"], json!([]));
        assert_eq!(done["removed"], json!(["_client/test"]));
        assert_eq!(done["vault_cleared"], true);
    }

    #[test]
    fn logout_by_org_matches_the_grants_id_or_name() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let mut settings = crate::config::ProfileSettings::default();
        settings.upsert(grant("pat-0001", "adult1"));
        let mut other = grant("pat-0002", "adult1");
        other.organization = "example.mmi".into();
        other.organization_name = Some("Example Dermatology".into());
        settings.upsert(other);
        let adult1 = ProfileName::parse("adult1").unwrap();
        ctx.save_settings(&adult1, &settings).unwrap();
        let at = |org: &str| LogoutArgs {
            who: PatientArgs {
                patient: None,
                org: Some(org.into()),
            },
            forget: false,
        };
        // A practice where nobody is signed in is not found, and signs nobody out.
        let err = signed_out(&ctx, &at("elsewhere.mmi")).unwrap_err();
        assert_eq!(err.exit_code(), 4, "{err}");
        assert!(err
            .remedy()
            .unwrap()
            .contains("patients list --profile adult1"));
        assert_eq!(
            ctx.settings(&ctx.config().unwrap(), &adult1)
                .unwrap()
                .grants
                .len(),
            2
        );
        let done = signed_out(&ctx, &at("example dermatology")).unwrap();
        assert_eq!(done["signed_out"], json!(["adult1"]));
        let left = ctx.settings(&ctx.config().unwrap(), &adult1).unwrap();
        assert_eq!(left.grants.len(), 1);
        assert_eq!(left.grants[0].organization, "test");
    }

    #[test]
    fn a_sign_in_that_ends_without_one_names_the_same_command() {
        let profile = ProfileName::parse("adult1").unwrap();
        let org = Org::test("http://127.0.0.1:9/fhir");
        let args = |label: Option<&str>, no_browser, timeout| LoginArgs {
            org: "test".into(),
            label: label.map(Into::into),
            no_browser,
            timeout,
        };
        assert_eq!(
            login_again(&profile, &org, &args(None, false, LOGIN_TIMEOUT)),
            "run `modmed auth login --profile adult1 --org test` again"
        );
        assert_eq!(
            login_again(&profile, &org, &args(Some("child1"), true, LOGIN_TIMEOUT)),
            "run `modmed auth login --profile adult1 --org test --label child1 --no-browser` again"
        );
        assert_eq!(
            login_again(&profile, &org, &args(None, false, 30)),
            "run `modmed auth login --profile adult1 --org test --timeout 30` again"
        );
        let kept = CliError::Auth("refused".into()).with_remedy("ask the practice");
        assert_eq!(or_again(kept, "again").remedy(), Some("ask the practice"));
        assert_eq!(
            or_again(CliError::Auth("timed out".into()), "again").remedy(),
            Some("again")
        );
    }

    #[test]
    fn config_is_written_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let mut settings = crate::config::ProfileSettings::default();
        settings.upsert(grant("pat-0001", "adult1"));
        ctx.save_settings(&ProfileName::parse("adult1").unwrap(), &settings)
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(ctx.store.path().unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn logout_forget_removes_both_clients_and_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        for environment in Environment::ALL {
            credentials::store_client(
                &ctx.vault,
                environment,
                &ClientCredentials::new("client-one", "secret-one").unwrap(),
            )
            .unwrap();
        }
        logout(&ctx, &forget()).unwrap();
        assert!(ctx.vault.names().unwrap().is_empty());
        // The profile stays configured: `profiles remove` removes it.
        assert!(ctx
            .config()
            .unwrap()
            .profiles
            .contains(&ProfileName::parse("adult1").unwrap()));
    }

    #[test]
    fn a_login_command_without_a_profile_is_a_usage_error_before_anything_else() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx(dir.path());
        ctx.profile = None;
        // MODMED_PROFILE is read too; no test sets it.
        let err = status_of(&ctx).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("configured: adult1"), "{err}");
        ctx.profile = Some("adult2".into());
        let err = status_of(&ctx).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert_eq!(
            err.remedy(),
            Some("add it with `modmed profiles add adult2`")
        );
    }

    #[test]
    fn the_test_organization_needs_a_test_server_configured() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let config = ctx.config().unwrap();
        let err = ctx.resolve_org(&config, "test").unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.remedy().unwrap().contains("test_fhir_base"));
        let mut config = config;
        config.test_fhir_base = Some("http://127.0.0.1:9/fhir".into());
        assert_eq!(ctx.resolve_org(&config, "TEST").unwrap().id, "test");
    }

    #[test]
    fn labels_are_short_and_plain() {
        for ok in ["adult1", "child-2", "a.b_c"] {
            check_label(ok).unwrap();
        }
        let long = "x".repeat(41);
        for bad in ["", "has space", "ünïcode", long.as_str()] {
            assert_eq!(check_label(bad).unwrap_err().exit_code(), 2, "{bad}");
        }
    }
}
