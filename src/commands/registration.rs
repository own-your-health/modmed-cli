//! `modmed registration`: what to enter on ModMed's vendor dashboard to
//! register the household's own app (`registration/v1`). Reads the config
//! only: no secret, no Keychain, no network.

use pk_cli_core::{output, CliError};

use super::{to_json, Ctx};
use crate::registration::{registration, REDIRECT_PAGE};

pub fn run(ctx: &Ctx) -> Result<(), CliError> {
    let config = ctx.config()?;
    let reg = registration(
        config.redirect_page.as_deref().unwrap_or(REDIRECT_PAGE),
        config.redirect_page.is_some(),
    );
    if ctx.json() {
        output::json(&to_json(&reg)?);
    } else {
        print!("{}", reg.text());
    }
    Ok(())
}
