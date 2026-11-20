//! `modmed registration [--test]`: what to enter to register a client
//! (`registration/v2`). Reads the config only: no secret, no Keychain, no
//! network.

use pk_cli_core::{output, CliError};

use super::{to_json, Ctx};
use crate::modmed::Environment;
use crate::registration::registration;

pub fn run(ctx: &Ctx, test: bool) -> Result<(), CliError> {
    let environment = if test {
        Environment::Test
    } else {
        Environment::Production
    };
    let reg = registration(environment, &ctx.config()?);
    if ctx.json() {
        output::json(&to_json(&reg)?);
    } else {
        print!("{}", reg.text());
    }
    Ok(())
}
