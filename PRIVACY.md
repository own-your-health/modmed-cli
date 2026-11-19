# Privacy

`modmed` is an open-source command-line program a patient runs on their own
computer to download their own health records from a practice that uses a
ModMed EHR. This page is its privacy policy, and the rule its repository is
kept by.

## What modmed reads

With the patient's consent, given on the practice's own sign-in and consent
pages, modmed reads through ModMed's Certified FHIR API:

- the patient's record, and their allergies, conditions, medications,
  results and observations, reports, procedures, immunizations, visits,
  documents and their files, care plans and care teams, goals, devices,
  orders, specimens, coverage and questionnaire answers;
- the practitioners, organizations, locations and medications those name.

It reads the records of the patient who signed in, or of a person the
patient's portal account may act for. It reads nothing else, and it never
changes anything at ModMed or the practice.

It also downloads ModMed's public directory of practices, which holds no
patient data.

## Where the data goes

modmed writes it only to the patient's own computer.

- modmed sends no record, and nothing about one, to its authors or to any
  other party. It has no server, no analytics, no telemetry, no crash
  reporting and no advertising.
- What modmed downloads is written to standard output or to files the
  patient names. Normally that output goes straight into another program
  the patient runs, such as `medrec import`, and where that program keeps
  records is the patient's choice and that program's policy, not modmed's.
- The sign-in tokens and the app's ModMed client id and secret are kept in
  modmed's vault: files under `~/.config/modmed/vault`, encrypted with a key
  kept in the operating system's credential store (the macOS Keychain),
  readable only by the patient's account.
- modmed keeps a settings file, readable only by the patient, under
  `~/.config/modmed`: its profiles, one per patient portal account, and for
  each the sign-ins it holds (each patient's label, practice and FHIR
  patient id). It holds no record. The practice directory is cached under
  `~/.cache/modmed`.
- Each profile signs in in its own Chrome profile, which keeps that portal
  account's cookies until the profile is removed.

modmed connects to ModMed's servers, and to GitHub only when the patient
asks it to update itself.

## Stopping access

`modmed auth logout --profile <NAME>` removes the sign-ins that profile
holds. `modmed profiles remove <NAME>` also removes its settings and its
Chrome profile. `modmed auth logout --profile <NAME> --forget`, once no
other profile holds a sign-in, also removes the app's client credentials and
the vault's key. To revoke modmed's access at the practice, the patient
turns it off in the practice's patient portal, where ModMed places a link to
manage the access after the first sign-in.

## Contact

Questions about this policy: open an issue at
https://github.com/piekstra/modmed-cli/issues. Never put a record or any
personal detail in an issue.

## The repository carries no personal data

Everything committed here is public. It holds:

- no record, response, document, export or directory capture from a real
  patient or practice (`.gitignore` blocks `*.ndjson`, `*.har` and capture
  directories);
- no name, identifier, address, phone number, email address or date
  belonging to a real person, and no practice the authors' household uses;
- no client id, client secret, token or cookie.

Test data is invented, with obviously fake values. `tests/no_personal_data.rs`
scans every tracked file for the shapes personal data takes,
`tests/fixture_shapes.rs` checks the fixtures' dummy values, and CI runs
gitleaks on every push and pull request.
