//! `modmed records export`.

use std::io::Write;

use pk_cli_core::{output, CliError};
use pk_med_fhir::FhirClient;
use pk_med_records::{create_export_file, Header, RecordsExport, RecordsWriter, Source};

use super::{report, rfc3339, sign_in_again, transport, Ctx, BIN};
use crate::cli::{ExportArgs, RecordsCmd};
use crate::export;
use crate::modmed::Environment;
use crate::session::{now, Context};

pub fn run(ctx: &Ctx, cmd: &RecordsCmd) -> Result<(), CliError> {
    let RecordsCmd::Export(args) = cmd;
    if let Some(e) = export(ctx, args)? {
        // The stream (or records-export/v1) is already the one document on
        // stdout, so the error goes to stderr and the process exits with
        // its code rather than printing a second document.
        report(&e);
        std::process::exit(e.exit_code());
    }
    Ok(())
}

fn validate(args: &ExportArgs, json: bool) -> Result<(), CliError> {
    if args.ndjson && json {
        return Err(CliError::Usage(
            "--ndjson is the output; leave out --json (it is for -o <DIR>, which prints records-export/v1)".into(),
        ));
    }
    if let Some(day) = &args.since {
        pk_cli_core::dates::parse_iso(day)?;
    }
    if !args.ndjson && args.output.is_none() {
        return Err(CliError::Usage(
            "say where the records go: --ndjson for stdout (what medrec reads when it runs modmed) or -o <DIR> (test servers only)".into(),
        ));
    }
    Ok(())
}

/// Whether a context's records are synthetic: the test organization, a
/// server on this machine ([`crate::modmed::require`], checked at sign-in
/// and again by `Live::start`). Only those go to a plain file.
fn synthetic(context: &Context) -> bool {
    context.environment() == Environment::Test
}

/// Run an export. `Err` is a failure before anything was written;
/// `Ok(Some(e))` one after the output began, which ended the stream with an
/// incomplete trailer.
pub fn export(ctx: &Ctx, args: &ExportArgs) -> Result<Option<CliError>, CliError> {
    validate(args, ctx.json())?;
    let config = ctx.config()?;
    let profile = ctx.profile(&config)?;
    let context = ctx.signed_in(&config, &profile, &args.who)?;
    // Refused before the Keychain is opened.
    if args.output.is_some() && !synthetic(&context) {
        // A label or a placeholder: the patient's FHIR id stays out of errors.
        let who = context.label.as_deref().unwrap_or("<WHO>");
        return Err(CliError::Usage(format!(
            "-o writes records to a plain file, so it is for test servers only; this patient is at {}",
            context.organization
        ))
        .with_remedy(format!(
            "take them on stdout with `{BIN} records export --profile {profile} --patient {who} --ndjson`, or let medrec run modmed and file them: `medrec --profile <FOLDER> import --from modmed --from-profile {profile} --patient {who}`"
        )));
    }
    let live = ctx.live(&profile, &context)?;
    let fhir_base = live.fhir_base();
    let t = transport()?;
    let source = live.clone();
    let mut fhir = FhirClient::new(&t, &fhir_base, BIN, env!("CARGO_PKG_VERSION"))?
        .bearer_with(move |refused| source.bearer(refused));
    if !ctx.common.quiet {
        fhir = fhir.on_retry(|line| eprintln!("{line}"));
    }
    let mut header = Header::new(
        Source::new(BIN, env!("CARGO_PKG_VERSION")),
        context.organization(&fhir_base),
        context.patient_context(),
        rfc3339(now()),
    );
    if let Some(day) = &args.since {
        header.since = Some(day.clone());
        header.since_unbounded = export::since_unbounded();
    }
    let verbose = ctx.common.verbose && !ctx.common.quiet;
    let note = move |line: String| {
        if verbose {
            eprintln!("{line}");
        }
    };
    let since = args.since.as_deref();

    match &args.output {
        None => {
            let stdout = std::io::stdout();
            let mut w = RecordsWriter::start(std::io::BufWriter::new(stdout.lock()), &header)?;
            let result = export::run(&fhir, &mut w, &context.id, since, &note)
                .map_err(|e| sign_in_again(e, &profile, &context));
            let finished = w.finish(rfc3339(now()), result.as_ref().err())?;
            if result.is_ok() {
                ctx.note(format!(
                    "exported {} resources for patient {} ({} skipped)",
                    finished.trailer.total(),
                    context.display(),
                    finished.trailer.skipped.len()
                ));
            }
            Ok(result.err())
        }
        Some(dir) => {
            let mut file = create_export_file(dir)?;
            let mut w = RecordsWriter::start(std::io::BufWriter::new(file.file()), &header)?;
            let result = export::run(&fhir, &mut w, &context.id, since, &note)
                .map_err(|e| sign_in_again(e, &profile, &context));
            let finished = w.finish(rfc3339(now()), result.as_ref().err())?;
            // A complete stream replaces records.ndjson; an incomplete one
            // goes beside it, and the last complete export stays.
            let path = file.commit(&finished)?;
            let payload = RecordsExport::new(path.display().to_string(), &finished);
            let v = serde_json::to_value(&payload).expect("records-export/v1 serializes");
            if ctx.json() {
                output::json(&v);
            } else {
                let _ = writeln!(
                    std::io::stdout(),
                    "{} {} resources to {} ({} skipped)",
                    if finished.trailer.complete {
                        "wrote"
                    } else {
                        "wrote an incomplete stream of"
                    },
                    finished.trailer.total(),
                    path.display(),
                    finished.trailer.skipped.len()
                );
            }
            Ok(result.err())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::PatientArgs;

    fn args(ndjson: bool, output: Option<&str>, since: Option<&str>) -> ExportArgs {
        ExportArgs {
            who: PatientArgs::default(),
            since: since.map(Into::into),
            ndjson,
            output: output.map(Into::into),
        }
    }

    #[test]
    fn the_stream_is_the_output_so_json_is_refused_with_it() {
        assert_eq!(
            validate(&args(true, None, None), true)
                .unwrap_err()
                .exit_code(),
            2
        );
        assert!(validate(&args(true, None, None), false).is_ok());
        assert!(validate(&args(false, Some("out"), None), true).is_ok());
        assert_eq!(
            validate(&args(false, None, None), false)
                .unwrap_err()
                .exit_code(),
            2
        );
        assert_eq!(
            validate(&args(true, None, Some("2024-13-01")), false)
                .unwrap_err()
                .exit_code(),
            2
        );
    }

    #[test]
    fn only_synthetic_records_go_to_a_plain_file() {
        let c = |org: &str| Context {
            id: "pat-0001".into(),
            organization: org.into(),
            organization_name: None,
            label: None,
            relationship: None,
            fhir_user: None,
            signed_in_at: "2020-01-01T00:00:00Z".into(),
        };
        assert!(synthetic(&c("test")));
        assert!(!synthetic(&c("example.mmi")));
    }
}
