# ModMed's Certified FHIR API

What `modmed` relies on, in ModMed's own words where it matters, with the
source of each fact and the traps. Read this before changing the sign-in,
the directory or the export.

Sources, read 2026-11-19:

- The API's documentation: `https://portal.api.modmed.com/docs/certified-fhir-api`
  (Getting started, Authentication, Reference).
- The Certified API Terms of Use, dated 2022-12-23:
  `https://www.modmed.com/wp-content/uploads/2023/01/Certified-API-Terms-of-Use-V2.pdf`.
- A practice's SMART configuration and CapabilityStatement, read from a
  directory entry's FHIR base (`<base>/.well-known/smart-configuration`,
  `<base>/metadata`); both are public.
- ModMed's public endpoint directories (below).

## Route

ModMed's EHRs (EMA, gGastro and the others) are ONC-certified, and ModMed
runs the certified patient API they require: FHIR R4, US Core, SMART App
Launch. "The ModMed Certified (ONC) FHIR API uses OAuth 2.0 with the SMART
App Launch framework." `modmed` is a standalone patient app on it, a
confidential client.

ModMed also has the "EMA Proprietary API" (an `x-api-key`, password and
client-credentials grants, for vendors in its synapSYS Marketplace, with a
technical review "before they are permitted to gain access to their first
customer's production system"), and Bulk FHIR (`$export`) for system apps.
Neither serves a patient's own app; `modmed` uses neither.

## Practices and the directory

Every practice has a FHIR base of its own:
`https://<practice>.<zone>.prod.fhir.ema-api.com/fhir/r4/`. ModMed
publishes two public directories, FHIR searchsets of `Endpoint` with each
one's managing `Organization`:

| Directory | First page |
|---|---|
| EMA and ModMed's other EHRs | `https://public-api.mmi.prod.fhir.ema-api.com/fhir/r4/Endpoint?_include=Endpoint:managingOrganization&connection-type=hl7-fhir-rest` |
| gGastro | `https://public-api.gastro.prod.fhir.ema-api.com/fhir/r4/Endpoint?_include=Endpoint:managingOrganization&connection-type=hl7-fhir-rest` |

The pages are 100 entries each and link the next with `link[relation=next]`
(`_offset=`). On 2026-11-19 the EMA directory had `total` 9056 endpoints
over 91 pages, 8759 organizations and 8782 distinct addresses; gGastro had
905. `Organization` carries `name`, `address` (city, state) and two
identifiers: an NPI (`http://hl7.org/fhir/sid/us-npi`) and the practice's
subdomain (an identifier whose `type.coding.system` is
`http://fhir.ema-api.com/ns/subdomain`).

`modmed orgs` downloads both into `~/.cache/modmed/orgs.json` and reads them
from there, downloading again when the copy is a week old (`orgs refresh
--force` sooner). A failed download keeps the old copy.

Traps:

- **The subdomain is not unique.** 27 subdomains appear in both the `ef` and
  `mmi` zones. An organization's id in `modmed` is therefore the host
  without `.prod.fhir.ema-api.com`: `<practice>.<zone>` (`example.mmi`).
  Every such id has a dot in it, so none collides with `test`, the test
  server's.
- **Endpoints repeat.** 274 addresses are listed more than once, one of the
  pair often managed by `Organization/1.null`, which the page does not
  include. `modmed` keeps one per id, the listing with a location and NPI.
- **Zones vary.** Most hosts are in `mmi.prod` (6064) or `ef.prod` (2714),
  with a few in others (`m-2.md.mmi`, `pc.mmi`); gGastro hosts are
  `<uuid>.gastro.prod`. Only https hosts under `fhir.ema-api.com` are kept.
- **`Endpoint.status` can be `off`.** Only `active` endpoints are listed.

## Sign-in

From the Authentication page:

- "Production only. The Certified FHIR API does not have a public sandbox.
  Authorization and API calls run against the production environment."
  `modmed`'s tests and dogfooding run on medical-common's mock server as the
  `test` organization instead.
- "Patient apps — Require a Patient at the practice (with valid Patient
  Portal username/password) to authenticate. After the first Standalone
  authentication, a link is placed inside the patient portal for repeated
  access and for the patient to manage or disable access." The sign-in is
  the practice's own patient portal account, so a household with two
  ModMed practices has a sign-in at each.
- "Redirect the user's browser to the authorization endpoint with:
  response_type=code, client_id, redirect_uri, scope (space-separated),
  state, aud (the FHIR base URL above), and code_challenge +
  code_challenge_method=S256."
- "authorization_code/refresh_token use client_secret_post (send
  client_id/client_secret in the body) ... PKCE (S256) is supported and
  recommended for user-facing apps." A practice's SMART configuration lists
  `token_endpoint_auth_methods_supported: ["client_secret_post"]` only.
  `modmed` sends the id and secret in the body (`ClientAuth::SecretPost`)
  and uses PKCE.
- "You receive an access_token (plus an id_token when openid is
  requested, and a refresh_token when online_access/offline_access is
  granted)."

The endpoints:

| | Where |
|---|---|
| Documented | `https://sso.ema.md/auth/realms/fhir/protocol/openid-connect/auth` and `.../token` |
| A practice's SMART configuration | `https://<practice>.<zone>.prod.fhir.ema-api.com/auth/realms/fhir/protocol/openid-connect/auth` and `.../token` |

`modmed` reads the practice's SMART configuration and accepts its endpoints
only on https hosts under `fhir.ema-api.com` or `sso.ema.md`, so the client
secret goes nowhere else (`modmed::require`).

### Scopes

ModMed's table, Standalone (patient picker): required "openid,
launch/patient", recommended "fhirUser, online_access, patient/*.rs".

- "`launch/patient` — Triggers patient picker" in a standalone launch.
- "`online_access` — Refresh token that expires when the user's EMA
  session ends". "`offline_access` — Refresh token that doesn't expire with
  the session". `modmed` asks for `offline_access`, so exports run without
  the patient present.
- The SMART configuration's `scopes_supported` lists no wildcard: 27
  explicit `patient/<Type>.rs` scopes, and `fhirUser`, `launch`,
  `offline_access`, `online_access`, `openid`, `profile`. `modmed` asks for
  each of the 27 by name (`modmed::PATIENT_TYPES`).
- Capabilities listed: `client-public`, `client-confidential-symmetric`,
  `permission-v1`, `permission-v2`, among others.

### Known only after the first real sign-in

These are recorded here when the registered app first signs in, with counts
and statuses only:

- how long the access and refresh tokens last, and whether refresh tokens
  rotate;
- whether a parent's portal account picks a child's record in the patient
  picker (`relationship: proxy` in `patients list`);
- whether the 27 explicit scopes are granted as asked, or the app must be
  registered with exactly the scopes it asks for;
- where a DocumentReference's file is (below).

## Registration

[registration.md](registration.md) walks through the vendor dashboard and
`modmed registration` prints its values. What the sign-in depends on:

- The dashboard takes one Redirect URL per app and refuses
  `http://127.0.0.1:7470/callback`: the create call
  (`/v1/vendor-app/create`) answers 400 "Content type 'text/html' not
  supported", every time. The app registers
  `https://piekstra.github.io/modmed-cli/callback` instead
  (`site/callback.html`, published by `.github/workflows/pages.yml`), which
  passes the query unchanged to `http://127.0.0.1:7470/callback`, so
  `redirect_port` stays 7470. With `modmed config set redirect_page
  https://piekstra.github.io/modmed-cli/callback`, modmed sends the page as
  `redirect_uri` in both the authorization and the token exchange.
- Access Type "Client-Confidential" with "Use PKCE S256" is a
  `client_secret_post` client with PKCE, as modmed signs in.
- The scopes are checkboxes: the 27 `patient/<Type>.rs`, and `openid`,
  `fhirUser`, `launch/patient` and `offline_access` among `online_access`,
  `launch`, `launch/encounter` and `profile`. The same 27 types are offered
  as `user/<Type>.rs`, for provider apps.
- A new app is "Disabled" until "Request to Enable App" and ModMed's
  approval ("Pending for Approval" in between). The dashboard shows no
  token or refresh-token lifetime.
- The registered client id and secret go in with `modmed auth
  set-credential` (a prompt that does not echo, `--stdin` or
  `--from-env`), into the vault entry `_client/production`.

## The Certified API Terms of Use

The points `modmed` is built around, quoted:

- 2.4: "You must protect your Credentials. Do not let anyone else use your
  Credentials and do not use anyone else's Credentials. ... Credentials may
  not be embedded in open source projects." The client id and secret live
  only in the owner's vault, never in this repository, a test or a log.
- 2.3.2: "If we offer a Test Environment ... you may only use test data."
  ModMed offers none for this API; `modmed` is tested on a local mock.
- 3.3.3: a Business Associate Agreement is needed "If you will be using the
  CAPIs to access or send Protected Health Information on behalf of ModMed
  or a ModMed Customer". `modmed` acts for the patient, on their own
  computer.
- 3.4.10: "Except as expressly authorized by these CAPI TOU, you will ...
  Not use any automated program, tool, or process (including web crawlers,
  scrapers, robots, bots, spiders, and automated scripts) to access the
  CAPI Services, the Systems ..." `modmed` uses the API the terms
  authorize; it never drives the practice's portal pages itself. The
  patient signs in and approves in a browser.
- 3.6: an Application must not contain code under a license that requires
  "our software code, or other software code combined or distributed with
  our software code, to be: (i) disclosed or distributed in source code
  form ...". `modmed` is MIT or Apache-2.0, and `deny.toml` admits no
  copyleft dependency.
- 4.2: "ModMed may monitor your use of the CAPI Services". "Currently,
  ModMed does not charge" for them.

## What `records export` reads

The CapabilityStatement ("Fhir Server 5.1.9_FINAL", FHIR 4.0.1) serves these
types, each searchable by `patient` unless noted:

| Type | Search parameters beside `patient` | `modmed` |
|---|---|---|
| AllergyIntolerance | `clinical-status` | search |
| CarePlan | `category`, `intent`, `status` | search, by `category=assess-plan` on a 400 |
| CareTeam | `status` | search, by each `status` on a 400 |
| Condition | `category`, `clinical-status`, `code`, `encounter`, `onset-date`, `recorded-date` | search |
| Coverage | | search |
| Device | `type` | search |
| DiagnosticReport | `category`, `code`, `date`, `status` | search, `date` under `--since`, by each US Core category on a 400 |
| DocumentReference | `category`, `date`, `period`, `status`, `type` | search, `date` under `--since`, by `category=clinical-note` on a 400 |
| Encounter | `class`, `date`, `identifier`, `status`, `type` | search, `date` under `--since` |
| Goal | `lifecycle-status`, `target-date` | search |
| Immunization | `date`, `status` | search, `date` under `--since` |
| MedicationDispense | | search |
| MedicationRequest | `authoredon`, `encounter`, `intent`, `status` | search, `authoredon` under `--since`, by each `intent` on a 400 |
| Observation | `category`, `code`, `date`, `status` | search, `date` under `--since`, by each US Core category on a 400 |
| Procedure | `code`, `date`, `status` | search, `date` under `--since` |
| QuestionnaireResponse | | search |
| RelatedPerson | `name` | search |
| ServiceRequest | `authored`, `category`, `code`, `status` | search, `authored` under `--since` |
| Specimen | | search |
| Medication | read only | read when referenced |
| Location, Organization, Practitioner, PractitionerRole | not by patient | read when referenced |
| Provenance | by `target` only | not read |
| Patient | | read first |

Traps:

- **No `_lastUpdated`.** No type lists it, so `--since` bounds only the
  types with a date parameter, by that date. The rest come whole, and the
  stream header's `since_unbounded` names them.
- **No `Binary`.** The CapabilityStatement lists no Binary type and
  `scopes_supported` no `patient/Binary.rs`. A DocumentReference attachment
  (or a DiagnosticReport `presentedForm`) is fetched only when its `url` is
  `Binary/<id>` on the practice's server; one carried inline in `data` is
  already in the resource; one anywhere else is a skip (`unsupported`), as
  is a Binary the server refuses. Which of these ModMed serves is known
  after the first real export.
- A refused search is a skip, not a stop: medical-common's skip codes, with
  the patient's id never in a skip's `search`.

## Errors

- A 401 on a FHIR read renews the token once (the family's refresh lock),
  then retries; a second refusal stops the export with exit 3 and `modmed
  auth login`'s command as the remedy.
- A 400 is `Upstream` with `Reason::BadRequest` (pk-med-fhir); `modmed`
  splits a search on it only for the types US Core lets a server require a
  parameter for.
- A refused or missing refresh token is exit 3; the patient signs in again
  at the practice.
