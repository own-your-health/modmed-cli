//! `modmed api GET <PATH>`: one FHIR read as the signed-in patient, the server's
//! answer on stdout.

use pk_cli_core::{block_on, CliError};
use pk_med_fhir::FhirClient;

use super::{sign_in_again, transport, Ctx, BIN};
use crate::cli::ApiArgs;

pub fn run(ctx: &Ctx, args: &ApiArgs) -> Result<(), CliError> {
    if !args.method.eq_ignore_ascii_case("GET") {
        return Err(CliError::Usage(
            "modmed reads and never writes: `api` takes GET only".into(),
        ));
    }
    if args.path.trim().is_empty() {
        return Err(CliError::Usage("give a path under the FHIR base".into()));
    }
    let config = ctx.config()?;
    let profile = ctx.profile(&config)?;
    let context = ctx.signed_in(&config, &profile, &args.who)?;
    let live = ctx.live(&profile, &context)?;
    let base = live.fhir_base();
    if args.path.contains("://") && !within(&args.path, &base) {
        return Err(CliError::Usage(format!(
            "`api` reads from the signed-in practice's FHIR server only: give a path under {base}"
        )));
    }
    let t = transport()?;
    let source = live.clone();
    let fhir = FhirClient::new(&t, &base, BIN, env!("CARGO_PKG_VERSION"))?
        .bearer_with(move |refused| source.bearer(refused));
    let body =
        block_on(fhir.get_raw(&args.path)).map_err(|e| sign_in_again(e, &profile, &context))?;
    match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(v) if !ctx.json() => println!(
            "{}",
            serde_json::to_string_pretty(&v).unwrap_or(body.clone())
        ),
        _ => println!("{}", body.trim_end()),
    }
    Ok(())
}

/// Whether `url` is `base` or under it: the same origin, and the path at a
/// segment boundary.
fn within(url: &str, base: &str) -> bool {
    let base = base.trim_end_matches('/');
    url.strip_prefix(base)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '?']))
}

#[cfg(test)]
mod tests {
    use super::within;

    #[test]
    fn only_a_url_under_the_fhir_base_is_within_it() {
        let base = "https://example.mmi.prod.fhir.ema-api.com/fhir/r4";
        assert!(within(&format!("{base}/Patient/x"), base));
        assert!(within(&format!("{base}?_format=json"), base));
        assert!(within(base, &format!("{base}/")));
        assert!(!within(&format!("{base}x/Patient"), base));
        assert!(!within("https://other.example/fhir/r4/Patient", base));
        assert!(!within(
            "https://example.mmi.prod.fhir.ema-api.com.other.example/fhir/r4",
            base
        ));
    }
}
