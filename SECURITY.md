# Security

## Reporting a vulnerability in modmed

Report it privately through GitHub's private vulnerability reporting
(Security, Report a vulnerability) on
https://github.com/piekstra/modmed-cli. Do not open a public issue, and do
not include a real record, patient detail, token or secret in the report.

## A flaw in ModMed's API

Report a flaw in ModMed's systems to ModMed, not here. ModMed's Certified
API Terms of Use (2.4) also ask to be told at once when credentials are lost
or stolen, or of any unauthorized use of the API.

## What modmed guarantees

- Credentials (the app's client id and secret, the patient's tokens) live
  only in modmed's vault: files encrypted with a key that is one item in the
  OS credential store under the service `piekstra.modmed`. They are never
  printed, logged, written to a file unencrypted, committed, or taken from
  the command line.
- modmed's own files (the config with each profile's patient ids, the
  vault, the directory cache) are owner-only and written whole.
- The client secret and the tokens go only to https hosts under ModMed's
  `fhir.ema-api.com` and `sso.ema.md`, checked on the parsed host before
  each sign-in and renewal; the `test` organization goes only to a server
  on this machine, with a client of its own.
- modmed reads; it never writes to ModMed.
- The test suite and `make verify` never read the real Keychain or reach
  ModMed (`MODMED_KEYCHAIN_SERVICE` points them at an empty service, and
  their config points the directory at this machine).
