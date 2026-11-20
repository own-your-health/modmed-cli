# Registering your own app with ModMed

`modmed` ships without a client id or secret. ModMed's terms keep
credentials out of open source projects (terms 2.4, below), so each
household registers its own patient app on ModMed's FHIR vendor dashboard
and stores the pair it is given in modmed's vault. The registration is
self-service and free ("Currently, ModMed does not charge", terms 4.2). The
forms take about 15 minutes. ModMed then reviews the app before it can sign
anyone in.

`modmed registration` prints the app form's values below (`--json`:
`registration/v2`), with the redirect from your own config. It reads no
secret and makes no request.

The labels are the ones version 3.0.0 of the dashboard shows. ModMed's own
guide to the choices is
<https://portal.api.modmed.com/docs/register-to-become-a-modmed-certified-fhir-api-vendor>.

## 1. Create a developer account

1. Open <https://fhir-vendor-dashboard.kube.prod.mmicse.com>, the
   **Register** button on ModMed's guide. It sends you to ModMed's sign-in
   at `sso.ema.md` ("Sign in to your account").
2. Choose **Register as New User**. The page is headed "Register". Fill it
   in as an individual: a household registering its own app is not a
   company.

   | Field | What to enter |
   |---|---|
   | First Name, Last Name | Your own. |
   | Email | Your own address. The verification link is sent here and expires within 5 minutes. |
   | Company Name | Your own name. |
   | Web page | `https://github.com/piekstra/modmed-cli`, the program you run. |
   | Street, City or Locality, State, Province, or Region, Zip or Postal code, Country | Your own address, for ModMed. |
   | Phone number | Optional; the form submits without it. |
   | Description of Company or Applications | For example: "An individual using the open-source modmed command-line program to download my own records from my ModMed practice to my own computer." |
   | Password, Confirm Password | A new password; keep it in your password manager. |

3. Submit. The page says "You need to verify your email address to activate
   your account." The email's subject is "Verify email", from ModMed's
   `practicemailer.com`, and its link "will expire within 5 minutes", so
   have the inbox open before you submit. The link signs you in on the
   dashboard.

The account opens at once. Terms 2.3.1: "As part of our review of your
registration, we may request additional information." Such a request comes
to the account's email.

## 2. Register the app

The dashboard is headed "FHIR Vendor Dashboard", with a "Certified Apps"
table (App Name, Type, Client ID, Secret, Status, Action). Choose
**Register New App**. The form is headed "App Details".

| Field | What to enter |
|---|---|
| Application Name | Required. Any name your household recognises, for example `modmed`. The practice's sign-in may show it to the patient. |
| Description | Optional. For example: "Downloads the patient's own records to their own computer. Reads only." |
| App Type | **Patient**. Not Provider (a practice's staff) and not Bulk (a practice's whole population). |
| FHIR Version | Shows v4.0.1; nothing to choose. |
| Access Type | **Client-Confidential**. modmed sends the client secret in the token request's body (`client_secret_post`). Not "Client-Confidential with JWKS". |
| PKCE | **Use PKCE S256**. |
| Launch URL | `https://github.com/piekstra/modmed-cli`. Required once a launch scope is checked: "Base (Launch) Url must be a valid url if a launch scope is present." modmed never uses it; it starts the sign-in itself (a standalone launch). |
| Redirect URL | `https://piekstra.github.io/modmed-cli/callback`, or your own copy of the page (section 5). One URL per app; [section 5](#5-what-the-dashboard-refuses) explains why it is not `http://127.0.0.1:7470/callback`. |
| Logo URL | Optional; leave empty. |
| Policy URL | `https://github.com/piekstra/modmed-cli/blob/main/PRIVACY.md`. |
| Terms of Service URL | Optional; leave empty. |

Each URL field asks "Please include http:// or https://".

### Scopes

Check exactly the 31 scopes modmed asks for at sign-in, and nothing else.
"Select All" in a group checks every box in it.

| Group on the form | Check | Leave unchecked |
|---|---|---|
| Standard Scopes (at least one ACL/Scope is required) | `openid`, `launch/patient` | `online_access`, `launch`, `launch/encounter` |
| Additional Scopes, Patient Scopes | all 27 `patient/<Type>.rs` (its "Select All") | none |
| Additional Scopes, User Scopes | none | all 27 `user/<Type>.rs` |
| Standard Scopes (the second group) | `fhirUser`, `offline_access` | `profile` |

The 27 types are AllergyIntolerance, CarePlan, CareTeam, Condition,
Coverage, Device, DiagnosticReport, DocumentReference, Encounter, Goal,
Immunization, Location, Medication, MedicationDispense, MedicationRequest,
Observation, Organization, Patient, Practitioner, PractitionerRole,
Procedure, Provenance, Questionnaire, QuestionnaireResponse, RelatedPerson,
ServiceRequest and Specimen, as `modmed registration` lists them.
`offline_access` is what lets modmed renew a sign-in without asking the
patient again.

Choose **Submit for Approval**. The app's row appears with Status
"Disabled".

## 3. Ask ModMed to enable it

1. In the app's row, choose the action **Request to Enable App**.
2. The dialog "Enable App Confirmation" asks "Are you sure you want to
   enable <name>?". Choose **Proceed**.
3. The page says "Request Submitted Successfully." and the row reads
   "Pending for Approval".

ModMed publishes no review time, and the dashboard sends no notice of its
own; check the row's Status, and the account's email for a request for more
information. The form also warns: "Please note that at first, there may be
a delay before all data is available". How long a sign-in at a practice
takes to work after approval is not published either.

## 4. Store the client id and secret

The row shows the Client ID and Secret in plain text. Store both with
`modmed auth set-credential`, in modmed's vault, which every profile and
practice shares:

```sh
modmed auth set-credential                 # asks for each without echoing it
op read "op://<vault>/<item>/pair" | modmed auth set-credential --stdin   # `<id>:<secret>`
{ op read "op://<vault>/<item>/client-id"; op read "op://<vault>/<item>/secret"; } \
  | modmed auth set-credential --stdin     # the two on separate lines
```

Never put either on a command line or in a file in a repository. Then point
modmed at the redirect page you registered:

```sh
modmed config set redirect_page https://piekstra.github.io/modmed-cli/callback
```

`modmed info --json` now lists the production client as configured
(`"clients": [{"registration": "production", "configured": true, ...},
...]`), and `modmed auth status --profile <NAME>` shows "client
(production): confidential (secret), in vault". Neither shows the id.
`modmed auth set-credential --overwrite` replaces a pair, for example
after ModMed issues a new secret.

## 5. What the dashboard refuses

| Tried | What happened | What to do |
|---|---|---|
| Redirect URL `http://127.0.0.1:7470/callback` | The create call answers 400 "Content type 'text/html' not supported", every time | Register the https page `https://piekstra.github.io/modmed-cli/callback` and set `redirect_page` to it (section 4) |
| A second Redirect URL | The form has one field | The page above is the only one |
| A launch scope with no Launch URL | "Base (Launch) Url must be a valid url if a launch scope is present." | Enter the repository's address |

The redirect page (`site/callback.html` in this repository) is static. It
passes the sign-in's answer, unchanged, to modmed's listener at
`http://127.0.0.1:7470/callback` on your own computer, so modmed still
listens on port 7470 (`redirect_port`). It loads nothing else and keeps
nothing. The answer's code is useless without the PKCE verifier and the
client secret, which never leave your computer. To host your own copy,
publish `site/callback.html` on any https address, register that address
instead, and set `redirect_page` to it.

## 6. The terms in short

The Certified API Terms of Use govern every app. [api.md](api.md) quotes
them; the points that matter here:

- 2.4: protect the client id and secret, let no one else use them, and
  never put them in an open source project.
- 2.3.1: ModMed may ask for more information while reviewing the
  registration.
- 3.3.3: a Business Associate Agreement is needed only when acting for
  ModMed or a practice. modmed acts for the patient, on the patient's own
  computer.
- 3.4.10: no automated program may access ModMed's systems beyond what the
  terms authorize. modmed uses the API only; the patient signs in and
  approves in a browser.
- 4.2: ModMed may monitor use, and does not charge for it.
