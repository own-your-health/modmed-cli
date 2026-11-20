# AGENTS.md — modmed-cli

`modmed` reads a patient's own health records from a ModMed practice (EMA,
gGastro and ModMed's other EHRs) through ModMed's Certified (ONC) FHIR API:
SMART App Launch, FHIR R4, US Core. It conforms to piekstra-cli/1 and
medical-common's `health-records/v1` and `documents/v1` profiles, on the
`pk-cli-*` crates from [cli-common](https://github.com/piekstra/cli-common)
and the `pk-med-*` crates from
[medical-common](https://github.com/piekstra/medical-common).

## Layout

| File | Owns |
|---|---|
| `src/main.rs` | parsing, dispatch, `info`, exit codes |
| `src/cli.rs` | the clap tree |
| `src/commands/*.rs` | one module per command group; `Ctx` in `mod.rs`: the config and its owner-only writes, `--profile`, the vault, the refresh lock, the directory cache, the sign-in window and `forget_browser`, `--org` resolution (`resolve_org` for a sign-in, `grant_org` among a profile's grants) |
| `src/modmed.rs` | where a sign-in may go (`require`, on the parsed host), the scopes, the two environments (`production`, `test`), `ModMedQuirks` |
| `src/orgs.rs` | ModMed's endpoint directories: paging, the organization id, de-duplication, the weekly cache, `resolve` and `search` |
| `src/session.rs` | a signed-in patient (`Context`, a profile's grant), picking one, the live token with renewal (`Live`, `client_secret_post`) |
| `src/export.rs` | what `records export` pulls, in order, the splits on a 400, attachments, and which failures skip rather than stop |
| `src/commands/documents.rs` | `documents list|download` over DocumentReference attachments |
| `src/config.rs` | the config: redirect port and page, the test server, the directory URL, the profiles and each one's grants |
| `src/credentials.rs` | the app's client id and secret: the vault entries `_client/production` and `_client/test` |
| `src/fsutil.rs` | owner-only files written whole |
| `site/` | GitHub Pages (`.github/workflows/pages.yml`): `callback.html`, the https page a practice may redirect a sign-in to, forwarding it to `http://127.0.0.1:7470/callback`; `tests/site.rs` checks its target and its CSP hashes |
| `examples/mock_modmed.rs` | `make mock`: medical-common's mock SMART + FHIR server and a synthetic directory, for dogfooding |
| `docs/api.md` | ModMed's API and terms, quoted, with sources and traps; read before changing auth, the directory or export |

## Medical data

Medical records are the most sensitive data a household has.

- The repository carries no personal data: invented test data only, checked
  by `tests/no_personal_data.rs`, `tests/fixture_shapes.rs` and gitleaks.
  No practice the household uses is named anywhere in it.
- ModMed has no sandbox for this API. Develop and dogfood against the mock
  (`make mock`, the `test` organization). A check against real data reports
  counts, statuses and exit codes only, or is left to the owner in their
  own terminal. An agent never reads, prints or summarizes a real record,
  document or patient name.
- No MCP surface: modmed is not registered in cli-mcp and ships no `mcp`
  command, because a tool call would hand records to the model making it.

## Credentials

- Profiles: one per patient portal account (`adult1`), in the config only.
  Every command that uses a sign-in (`auth login|status|logout`,
  `patients`, `records`, `documents`, `api`) needs `--profile <NAME>` or
  `MODMED_PROFILE`; there is no default, and a lone profile is never picked
  (exit 2, listing them). `orgs`, `profiles`, `config`, `auth
  set-credential` and `info` take none.
- Every secret is in modmed's vault (cli-common `pk_cli_auth::vault`):
  encrypted files in `~/.config/modmed/vault/` under one Keychain item, the
  key `vault-key` in service `piekstra.modmed`. A newly built modmed asks
  macOS once. Install and upgrade modmed only when the owner is present.
- ModMed registers the app as a confidential client. Its client id and
  secret are the vault entry `_client/production`, stored by `modmed auth
  set-credential`; `_client/test` (`--test`) is the mock's, so ModMed's
  secret never goes to a test server. ModMed's terms (2.4) say credentials
  "may not be embedded in open source projects": never printed, logged,
  committed or put in argv.
- A patient's sign-in is pk-med-smart's session, the vault entry
  `<profile>/<org>/<FHIR patient id>`. Errors never name it. Renewal runs
  under the family's refresh lock.
- The sign-in opens Chrome in the profile's own browser profile
  (`SignInWindow`), so one portal account's cookies never sign in another;
  `profiles remove` deletes it. The patient signs in and approves; modmed
  never drives the portal's pages (terms 3.4.10).
- Tests and `make verify` never read the real Keychain or the network:
  `MODMED_KEYCHAIN_SERVICE` names an empty service, `MODMED_CACHE_DIR` a
  scratch cache, and the surface tests' config points the directory at a
  closed port or a local stand-in.
- If a `modmed` call hangs, it waits on a Keychain approval. Stop calling it
  and tell the owner; a retry only raises another dialog.

## Conventions

- `--json` on every command, one schema-tagged document on stdout, except
  `records export --ndjson`, whose stream is the output.
- Arguments, and secrets from `--stdin` or `--from-env`, are checked before
  the Keychain or the network.
- Read-only: modmed never writes to ModMed; `api` takes GET only.
- `records export -o` writes only the `test` organization's records to a
  file and refuses before the Keychain otherwise: a real patient's records
  go to stdout, which medrec reads when `medrec import --from modmed` runs
  modmed. Errors name a patient by label or `<WHO>`, never by FHIR id.
- Where a sign-in may go is an allowlist (`modmed::require`, on the parsed
  host): a practice to https hosts under `fhir.ema-api.com` (its FHIR base)
  or `sso.ema.md` (sign-in), the `test` organization to this machine only.
  `auth login`, `records export`, `documents` and `api` check again before
  a renewal sends the secret.
- An organization id is the FHIR host without `.prod.fhir.ema-api.com`
  (`example.mmi`); `--org` on `auth login` also takes a subdomain, NPI or
  name, resolved against the cached directory.
- Every binary-producing `make` target signs with the stable
  `pk-cli-codesign` identity.

## Tests

`make verify` (fmt, clippy, tests, smoke), offline:

- `tests/cli_surface.rs`: the binary's help, exit codes, JSON errors,
  `info`, completions, `config`, `profiles`, `orgs` over a local directory
  (paging, the weekly cache, `--force`, a stale copy, no copy), a login
  command refused without a configured `--profile`, logged-out status.
- `tests/e2e.rs`: in process against medical-common's mock server as the
  `test` organization, with a stand-in browser and a vault over
  `MemoryKeys`: sign-in (SMART configuration, `client_secret_post`, the
  scopes, `aud`), export counts and order, `--since`, a 400 split by
  category, refused and failing searches and Binaries as skips, withheld
  results, a refused token renewed once, a refused renewal, a declined
  consent, `-o` refused for a practice's patient, two patients and logout,
  a sign-in through a forwarding page, a practice that needs the production
  client, sign-ins pointing elsewhere, documents list and download, and a
  document that fails its check.
- `tests/fixture_shapes.rs`: the directory fixtures keep their dummy shapes
  and parse.
- `tests/site.rs`: the forwarding page's target and Content-Security-Policy.
- `tests/no_personal_data.rs`: personal-data shapes in tracked files.
- Unit tests in each module.

## Definition of done

`make verify` and CI green, the installed binary dogfooded against the
mock, `docs/api.md` matching what ModMed does, and no personal data
anywhere in the diff.
