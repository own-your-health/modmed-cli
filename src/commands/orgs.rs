//! `modmed orgs refresh|list|search`: the practices in ModMed's endpoint
//! directories, as org-list/v1.

use pk_cli_core::{output, CliError};
use pk_med_records::org_list;
use serde_json::json;

use super::{rfc3339, Ctx};
use crate::cli::{OrgListArgs, OrgsCmd};
use crate::orgs::{self, Org};

pub fn run(ctx: &Ctx, cmd: &OrgsCmd) -> Result<(), CliError> {
    match cmd {
        OrgsCmd::Refresh { force } => refresh(ctx, *force),
        OrgsCmd::List(list) => {
            validate(list)?;
            let index = ctx.org_index(&ctx.config()?, false)?;
            emit(ctx, index.orgs.iter().collect(), list);
            Ok(())
        }
        OrgsCmd::Search { text, list } => {
            validate(list)?;
            if text.trim().is_empty() {
                return Err(CliError::Usage("give some text to search for".into()));
            }
            let index = ctx.org_index(&ctx.config()?, false)?;
            let hits = orgs::search(&index.orgs, text);
            if hits.is_empty() {
                return Err(CliError::NotFound(format!(
                    "no practice in ModMed's directories matches `{text}`"
                ))
                .with_remedy("try fewer words, the practice's city, or its NPI"));
            }
            emit(ctx, hits, list);
            Ok(())
        }
    }
}

fn validate(list: &OrgListArgs) -> Result<(), CliError> {
    if let Some(st) = &list.state {
        if st.len() != 2 || !st.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(CliError::Usage(format!("`{st}` is not a two-letter state")));
        }
    }
    if list.limit == Some(0) {
        return Err(CliError::Usage("--limit must be at least 1".into()));
    }
    Ok(())
}

fn emit(ctx: &Ctx, orgs: Vec<&Org>, list: &OrgListArgs) {
    let state = list.state.as_deref().map(str::to_ascii_uppercase);
    let mut items: Vec<pk_med_records::Organization> = orgs
        .into_iter()
        .filter(|o| state.is_none() || o.state == state)
        .map(Org::organization)
        .collect();
    let total = items.len();
    if let Some(n) = list.limit {
        items.truncate(n);
    }
    let shown = items.len();
    let v = serde_json::to_value(org_list(items)).expect("org-list/v1 serializes");
    if ctx.json() {
        output::json(&v);
        return;
    }
    let rows = output::rows_of(&v, "items");
    if rows.is_empty() {
        ctx.note("no practice matches");
    }
    output::table(&output::table_view(&rows, &["id", "name", "location"]));
    if shown < total {
        ctx.note(format!("{shown} of {total} shown (--limit)"));
    }
}

fn refresh(ctx: &Ctx, force: bool) -> Result<(), CliError> {
    let (index, refreshed) = ctx.org_index_downloaded(&ctx.config()?, force)?;
    let payload = json!({
        "sources": index.sources,
        "fetched_at": rfc3339(index.fetched_at.max(0) as u64),
        "organizations": index.orgs.len(),
        "refreshed": refreshed,
        "next_refresh_after": rfc3339((index.fetched_at + orgs::REFRESH_EVERY_SECS).max(0) as u64),
        "cache": orgs::index_path(&ctx.cache).display().to_string(),
    });
    output::emit(ctx.json(), "org-refresh", payload, |p| {
        println!(
            "{} practices{}; next refresh after {}",
            p["organizations"],
            if refreshed {
                ", downloaded now"
            } else {
                ", cached copy is current"
            },
            p["next_refresh_after"].as_str().unwrap_or_default()
        );
    });
    Ok(())
}
