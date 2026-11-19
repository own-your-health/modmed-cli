//! `modmed patients list`: the patients signed in, as patient-list/v1.

use pk_cli_core::{output, CliError};
use pk_med_records::patient_list;
use pk_med_smart::SessionStore;
use serde_json::Value;

use super::{Ctx, BIN};
use crate::cli::PatientsCmd;
use crate::session::now;

pub fn run(ctx: &Ctx, cmd: &PatientsCmd) -> Result<(), CliError> {
    let PatientsCmd::List { status, org } = cmd;
    let list = list(ctx, *status, org.as_deref())?;
    if ctx.json() {
        output::json(&list);
        return Ok(());
    }
    let rows = output::rows_of(&list, "items");
    if rows.is_empty() {
        let profile = ctx.profile(&ctx.config()?)?;
        match org {
            Some(org) => println!("No patient is signed in at `{org}` in profile {profile}."),
            None => println!(
                "No patient is signed in in profile {profile} (`{BIN} auth login --profile {profile} --org <ORG> --label <WHO>`)."
            ),
        }
        return Ok(());
    }
    let mut columns = vec!["label", "id", "organization", "relationship"];
    if *status {
        columns.extend(["usable", "refused", "error", "remedy"]);
    }
    output::table(&output::table_view(&rows, &columns));
    Ok(())
}

/// `patient-list/v1` for the profile's patients `org` selects (a grant's
/// organization id or name). With `status`, each row also has `usable` and
/// `refused` (whether the practice refused the sign-in). A sign-in that
/// cannot be read is that row's `error`, not the command's, with `usable:
/// false` and `refused: null`, since it is unknown, and its `remedy` as
/// modmed's command.
pub fn list(ctx: &Ctx, status: bool, org: Option<&str>) -> Result<Value, CliError> {
    let config = ctx.config()?;
    let profile = ctx.profile(&config)?;
    let grants = ctx.settings(&config, &profile)?.grants;
    let org = org.map(|q| super::grant_org(&grants, q).to_string());
    let contexts: Vec<_> = grants
        .iter()
        .filter(|c| org.as_deref().is_none_or(|o| c.organization == o))
        .cloned()
        .collect();
    let sessions = ctx.sessions(&profile);
    let mut list = serde_json::to_value(patient_list(
        contexts.iter().map(|c| c.patient_context()).collect(),
    ))
    .expect("patient-list/v1 serializes");
    if !status {
        return Ok(list);
    }
    if let Some(rows) = list["items"].as_array_mut() {
        for (row, c) in rows.iter_mut().zip(&contexts) {
            match sessions.load(&c.account()) {
                Ok(session) => {
                    // A token the practice refused is never handed out again; the
                    // next use renews it, so the sign-in is usable only if
                    // it can.
                    let usable = session.as_ref().is_some_and(|s| {
                        (s.refused_at.is_none() && !s.tokens.is_expired(now(), 0)) || s.renewable()
                    });
                    row["usable"] = usable.into();
                    row["refused"] = session
                        .as_ref()
                        .is_some_and(|s| s.refused_at.is_some())
                        .into();
                }
                Err(e) => {
                    let e = super::sign_in_again(e, &profile, c);
                    row["usable"] = false.into();
                    row["refused"] = Value::Null;
                    row["error"] = e.message().into();
                    if let Some(remedy) = e.remedy() {
                        row["remedy"] = remedy.into();
                    }
                }
            }
        }
    }
    Ok(list)
}
