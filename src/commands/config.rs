//! `modmed config path|show|set|unset`.

use pk_cli_core::{output, CliError};

use super::Ctx;
use crate::cli::ConfigCmd;
use crate::config::Config;

pub fn run(ctx: &Ctx, cmd: &ConfigCmd) -> Result<(), CliError> {
    match cmd {
        ConfigCmd::Path => {
            let path = ctx.store.path()?.display().to_string();
            output::emit(
                ctx.json(),
                "config-path",
                serde_json::json!({ "path": path }),
                |p| println!("{}", p["path"].as_str().unwrap_or_default()),
            );
            Ok(())
        }
        ConfigCmd::Show => {
            let cfg: Config = ctx.config()?;
            let effective = serde_json::json!({
                "redirect_uri": cfg.redirect_uri(),
                "redirect_port": cfg.redirect_port(),
                "test_fhir_base": cfg.test_fhir_base,
                "directories": cfg.directories().into_iter().map(|(_, u)| u).collect::<Vec<_>>(),
                "cache": crate::orgs::index_path(&ctx.cache).display().to_string(),
            });
            output::emit(ctx.json(), "config", effective, output::render);
            Ok(())
        }
        ConfigCmd::Set { key, value } => {
            let mut cfg: Config = ctx.config()?;
            cfg.set(key, value)?;
            ctx.save_config(&cfg)?;
            ctx.note(format!("{key} set"));
            Ok(())
        }
        ConfigCmd::Unset { key } => {
            let mut cfg: Config = ctx.config()?;
            cfg.unset(key)?;
            ctx.save_config(&cfg)?;
            ctx.note(format!("{key} unset"));
            Ok(())
        }
    }
}
