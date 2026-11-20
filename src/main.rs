use clap::{CommandFactory, Parser};
use pk_cli_core::info::{AuthInfo, CliInfo};
use pk_cli_core::{output, CliError};
use pk_cli_selfupdate::Updater;

use modmed::cli::{Cli, Command};
use modmed::commands::{self, Ctx, BIN, REPO};

/// The profiles `info` declares.
const PROFILES: &[&str] = &["health-records/v1", "documents/v1"];

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        std::process::exit(output::fail(&e, cli.common.json));
    }
}

fn run(cli: &Cli) -> Result<(), CliError> {
    if cli.profile.is_some() && !cli.command.uses_profile() {
        return Err(CliError::Usage(
            "this command does not run as a profile, so it takes no --profile".into(),
        )
        .with_remedy(match &cli.command {
            Command::Profiles(_) => {
                "name the profile as the argument: `modmed profiles remove <NAME>`"
            }
            _ => "leave out --profile",
        }));
    }
    let ctx = Ctx::for_binary(cli.common.clone(), cli.config.clone(), cli.profile.clone())?;
    match &cli.command {
        Command::Orgs(cmd) => commands::orgs::run(&ctx, cmd),
        Command::Profiles(cmd) => commands::profiles::run(&ctx, cmd),
        Command::Auth(cmd) => commands::auth::run(&ctx, cmd),
        Command::Patients(cmd) => commands::patients::run(&ctx, cmd),
        Command::Records(cmd) => commands::records::run(&ctx, cmd),
        Command::Documents(cmd) => commands::documents::run(&ctx, cmd),
        Command::Api(args) => commands::api::run(&ctx, args),
        Command::Config(cmd) => commands::config::run(&ctx, cmd),
        Command::Registration { test } => commands::registration::run(&ctx, *test),
        Command::SelfUpdate(args) => Updater {
            repo: REPO.into(),
            binary: BIN.into(),
            target: env!("BUILD_TARGET").into(),
            current: env!("CARGO_PKG_VERSION").into(),
        }
        .run(args, cli.common.json, cli.common.quiet),
        Command::Completions { shell } => {
            // Rendered whole first: clap_complete panics when stdout closes
            // early (`| head`), and a closed pipe is not an error.
            let mut script = Vec::new();
            clap_complete::generate(*shell, &mut Cli::command(), BIN, &mut script);
            match std::io::Write::write_all(&mut std::io::stdout(), &script) {
                Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(CliError::Other(
                    format!("could not write the completion script: {e}"),
                )),
                _ => Ok(()),
            }
        }
        Command::Info => {
            let mut info = CliInfo::new(
                BIN,
                env!("CARGO_PKG_VERSION"),
                &format!("https://github.com/{REPO}"),
                AuthInfo {
                    required: true,
                    method: "browser-session".into(),
                    login_hint: Some(format!(
                        "{BIN} registration (an app of your own: {}), {BIN} auth set-credential, {BIN} profiles add <NAME>, {BIN} orgs search <WORDS>, then {BIN} auth login --profile <NAME> --org <ORG> --label <WHO>",
                        modmed::registration::DOC
                    )),
                },
                &[
                    "orgs",
                    "profiles",
                    "auth",
                    "patients",
                    "records",
                    "documents",
                    "api",
                    "config",
                    "registration",
                ],
            )
            .with_profiles(PROFILES);
            // Login commands take --profile, so a records store passes one.
            info.capabilities
                .push(pk_med_records::LOGIN_PROFILES.to_string());
            info.capabilities
                .push(pk_med_records::REGISTRATION.to_string());
            // Which clients are stored, from the vault's file names: no
            // Keychain read, since a records store runs `info` unattended.
            let clients = modmed::registration::client_statuses(&ctx.vault, &ctx.config()?)?;
            output::json(&pk_med_smart::with_clients(&info, &clients)?);
            Ok(())
        }
    }
}
