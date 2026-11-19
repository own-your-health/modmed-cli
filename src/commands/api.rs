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
    if absolute(&args.path) && !within(&args.path, &base) {
        return Err(CliError::Usage(format!(
            "`api` reads from the signed-in practice's FHIR server only: give a path under {base}"
        )));
    }
    let t = transport()?;
    let source = live.clone();
    let fhir = FhirClient::new(&t, &base, BIN, env!("CARGO_PKG_VERSION"))?
        .bearer_with(move |refused| source.bearer(refused));
    let path = if absolute(&args.path) {
        args.path.clone()
    } else {
        query_urls_escaped(&args.path)
    };
    let body = block_on(fhir.get_raw(&path)).map_err(|e| sign_in_again(e, &profile, &context))?;
    match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(v) if !ctx.json() => println!(
            "{}",
            serde_json::to_string_pretty(&v).unwrap_or(body.clone())
        ),
        _ => println!("{}", body.trim_end()),
    }
    Ok(())
}

/// Whether `path` names a server: it starts with a scheme (`https://`) or
/// `//`. A `://` later on, as in a token search's system
/// (`code=http://snomed.info/sct|123`), is part of a relative path.
fn absolute(path: &str) -> bool {
    let path = path.trim_start();
    if path.starts_with("//") {
        return true;
    }
    path.split_once("://").is_some_and(|(scheme, _)| {
        scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// `path` with each `://` in its query as `%3A//`, which the server decodes
/// back: pk-med-fhir takes any path holding `://` for a URL on another
/// server, and a token search's system is one
/// (`Condition?code=http://snomed.info/sct|123`).
fn query_urls_escaped(path: &str) -> String {
    match path.split_once('?') {
        Some((resource, query)) => format!("{resource}?{}", query.replace("://", "%3A//")),
        None => path.to_string(),
    }
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
    use super::{absolute, query_urls_escaped, within};

    #[test]
    fn a_url_in_the_query_is_escaped_and_the_rest_kept() {
        assert_eq!(
            query_urls_escaped("Condition?patient=p1&code=http://snomed.info/sct|123"),
            "Condition?patient=p1&code=http%3A//snomed.info/sct|123"
        );
        assert_eq!(query_urls_escaped("Patient/p1"), "Patient/p1");
        assert_eq!(
            query_urls_escaped("Condition?patient=p1"),
            "Condition?patient=p1"
        );
    }

    #[test]
    fn a_path_is_absolute_only_when_it_starts_with_a_scheme() {
        assert!(absolute("https://elsewhere.example/fhir/Patient"));
        assert!(absolute("HTTP://127.0.0.1:1/fhir"));
        assert!(absolute("//elsewhere.example/fhir/Patient"));
        assert!(!absolute("Condition?code=http://snomed.info/sct|123"));
        assert!(!absolute("Patient?identifier=http://example.org/mrn|1"));
        assert!(!absolute("Patient/pat-0001"));
        assert!(!absolute("metadata"));
    }

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
