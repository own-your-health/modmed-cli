//! `modmed profiles list|add|remove`: the logins modmed keeps, one per
//! patient portal account. Names are the household's own (`adult1`) and
//! live in the config only.

use pk_cli_config::profile::{self, ProfileName};
use pk_cli_core::{confirm, output, CliError};
use serde_json::{json, Value};

use super::{Ctx, BIN, PROFILE_ENV};
use crate::cli::ProfilesCmd;

pub fn run(ctx: &Ctx, cmd: &ProfilesCmd) -> Result<(), CliError> {
    match cmd {
        ProfilesCmd::List => list(ctx),
        ProfilesCmd::Add { name } => add(ctx, name),
        ProfilesCmd::Remove { name, force } => remove(ctx, name, *force),
    }
}

fn list(ctx: &Ctx) -> Result<(), CliError> {
    let list = ctx.config()?.profiles.list();
    if ctx.json() {
        output::json(&serde_json::to_value(&list).expect("profile-list/v1 serializes"));
    } else if list.profiles.is_empty() {
        println!("No profile is configured (`{BIN} profiles add <NAME>`).");
    } else {
        for p in &list.profiles {
            println!("{}", p.name);
        }
    }
    Ok(())
}

fn add(ctx: &Ctx, name: &str) -> Result<(), CliError> {
    let done = added(ctx, name)?;
    let name = done["profile"].as_str().unwrap_or_default().to_string();
    output::emit(ctx.json(), "profile-add", done.clone(), |_| {
        if done["added"] == true {
            println!("Added profile {name}. Sign its patients in with `{BIN} auth login --profile {name} --org <ORG> --label <WHO>`.");
        } else {
            println!("Profile {name} is already configured.");
        }
    });
    Ok(())
}

/// `profile-add/v1`'s payload: `profile`, and `added`, false when it was
/// already configured.
fn added(ctx: &Ctx, name: &str) -> Result<Value, CliError> {
    let name = ProfileName::parse(name)?;
    let mut config = ctx.config()?;
    let added = config.profiles.add(name.clone());
    if added {
        ctx.save_config(&config)?;
    }
    Ok(json!({ "profile": name.as_str(), "added": added }))
}

fn remove(ctx: &Ctx, name: &str, force: bool) -> Result<(), CliError> {
    let done = removed(ctx, name, force)?;
    output::emit(ctx.json(), "profile-remove", done.clone(), |_| {
        println!(
            "Removed profile {} and its {} sign-in(s).",
            done["profile"].as_str().unwrap_or_default(),
            done["sign_ins_removed"]
        )
    });
    Ok(())
}

/// Its sign-ins, its kept browser profile, then its config, after asking.
/// The config goes last, so a step that fails leaves the profile listed
/// and running the command again finishes it. `profile-remove/v1`'s
/// payload: `profile`, `sign_ins_removed`, `browser_profile_removed`.
fn removed(ctx: &Ctx, name: &str, force: bool) -> Result<Value, CliError> {
    let config = ctx.config()?;
    // An unknown name is the same usage error `--profile` gives.
    let name = profile::resolve_from(Some(name), None, PROFILE_ENV, &config.profiles, BIN)?;
    confirm::gate(
        &ctx.common,
        force,
        &format!("Remove profile {name}, its sign-ins and its browser profile?"),
    )?;
    let again = format!("run `{BIN} profiles remove {name} --force` again to finish");
    let sessions = ctx.sessions(&name);
    let sign_ins = ctx
        .lock
        .forget(|| sessions.delete_all())
        .map_err(|e| e.with_remedy(again.clone()))?;
    let browser_profile = (ctx.forget_browser)(&name).map_err(|e| e.with_remedy(again))?;
    let mut config = ctx.config()?;
    config.profiles.remove(&name);
    ctx.save_config(&config)?;
    Ok(json!({
        "profile": name.as_str(),
        "sign_ins_removed": sign_ins,
        "browser_profile_removed": browser_profile,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::auth::tests::ctx;
    use pk_med_smart::SessionStore;

    /// A stored sign-in as pk-med-smart encodes it.
    fn stored_session(id: &str) -> String {
        json!({
            "format": 1,
            "organization": "test",
            "patient": id,
            "fhir_base": "http://127.0.0.1:9/fhir",
            "issuer": null,
            "token_endpoint": "http://127.0.0.1:9/token",
            "client_id": "client-one",
            "scope": "patient/*.read",
            "tokens": { "access_token": "a", "token_type": "Bearer", "obtained_at": 1 },
        })
        .to_string()
    }

    #[test]
    fn remove_takes_the_sign_ins_and_the_config_and_leaves_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        add(&ctx, "adult2").unwrap();
        let adult1 = ProfileName::parse("adult1").unwrap();
        let adult2 = ProfileName::parse("adult2").unwrap();
        let session = pk_med_smart::decode_session(&stored_session("pat-0001")).unwrap();
        ctx.sessions(&adult1).save(&session).unwrap();
        ctx.sessions(&adult2).save(&session).unwrap();
        assert_eq!(
            removed(&ctx, "adult1", true).unwrap(),
            json!({"profile": "adult1", "sign_ins_removed": 1, "browser_profile_removed": false})
        );
        let config = ctx.config().unwrap();
        assert!(!config.profiles.contains(&adult1));
        assert!(config.profiles.contains(&adult2));
        assert!(ctx.sessions(&adult1).accounts().unwrap().is_empty());
        assert_eq!(ctx.sessions(&adult2).accounts().unwrap().len(), 1);
        assert_eq!(remove(&ctx, "adult1", true).unwrap_err().exit_code(), 2);
    }

    #[test]
    fn remove_without_force_off_a_terminal_asks_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx(dir.path());
        // `--json` never asks, whatever stdin is.
        ctx.common.json = true;
        assert_eq!(remove(&ctx, "adult1", false).unwrap_err().exit_code(), 6);
        assert!(ctx
            .config()
            .unwrap()
            .profiles
            .contains(&ProfileName::parse("adult1").unwrap()));
    }

    #[test]
    fn add_says_whether_it_added() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        assert_eq!(
            added(&ctx, "adult2").unwrap(),
            json!({"profile": "adult2", "added": true})
        );
        assert_eq!(added(&ctx, "adult2").unwrap()["added"], false);
    }

    #[test]
    fn a_remove_whose_browser_profile_stays_keeps_the_profile_and_says_to_run_it_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx(dir.path());
        ctx.forget_browser = Box::new(|_| Err(CliError::Other("Chrome holds the profile".into())));
        let err = removed(&ctx, "adult1", true).unwrap_err();
        assert_eq!(
            err.remedy(),
            Some("run `modmed profiles remove adult1 --force` again to finish")
        );
        assert!(ctx
            .config()
            .unwrap()
            .profiles
            .contains(&ProfileName::parse("adult1").unwrap()));
    }

    #[test]
    fn a_profile_name_is_checked() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        for bad in ["", "Adult1", "_client", "has space", "a/b"] {
            assert_eq!(add(&ctx, bad).unwrap_err().exit_code(), 2, "{bad}");
        }
    }
}
