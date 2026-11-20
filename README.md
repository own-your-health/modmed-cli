# modmed — your records from a ModMed practice

`modmed` downloads your own health records from a practice that runs a
ModMed EHR (EMA, gGastro and ModMed's other systems) through ModMed's
Certified (ONC) FHIR API: conditions, medications, results, notes and their
files, visits, immunizations, care plans and the rest. It runs on your
computer and sends nothing anywhere else ([PRIVACY.md](PRIVACY.md)).

Unofficial; not affiliated with or endorsed by ModMed (Modernizing
Medicine, Inc.).

## Who it is for

Anyone with a patient portal account at a practice that runs a ModMed EHR.
`modmed orgs search` finds the practice in ModMed's public directory. There
is no shared app to sign in with: ModMed's terms keep an app's client id and
secret out of open source projects, so each household registers its own app
once (about 15 minutes of forms, then ModMed's review) and keeps the pair in
modmed's vault.

## Status

v0.2.0. The sign-in, `records export` and `documents` are tested against
medical-common's mock server; ModMed has no sandbox for this API, so the
first real sign-in comes with a registered app. [docs/api.md](docs/api.md)
lists what is known only after it.

## Install

Pick the target (`aarch64-apple-darwin` for Apple Silicon,
`x86_64-apple-darwin` for an Intel Mac, `x86_64-unknown-linux-gnu`), check
the archive against its `.sha256`, then put `modmed` on your PATH:

```sh
t=aarch64-apple-darwin
u=https://github.com/piekstra/modmed-cli/releases/latest/download/modmed-$t.tar.gz
curl -sSLO "$u" && curl -sSLO "$u.sha256"
shasum -a 256 -c "modmed-$t.tar.gz.sha256"    # on Linux: sha256sum -c
tar xzf "modmed-$t.tar.gz" && mkdir -p ~/.local/bin && mv modmed ~/.local/bin/
modmed self-update                            # later releases
```

From source: `make install`.

## Register your app (once)

[docs/registration.md](docs/registration.md) walks through ModMed's FHIR
vendor dashboard step by step, and `modmed registration` prints every value
to enter (`--json` for a program). In short: a developer account, then a
Patient app, Client-Confidential, PKCE S256, the 31 scopes modmed asks for,
and the redirect URL `https://piekstra.github.io/modmed-cli/callback`, a page
that passes the sign-in's answer to modmed on your computer (the dashboard
refuses `http://127.0.0.1`). Then "Request to Enable App" and wait for
ModMed's approval.

## Quick start

```sh
# The app's pair, from the dashboard (asks for each without echoing it),
# and the redirect page it was registered with.
modmed auth set-credential
modmed config set redirect_page https://piekstra.github.io/modmed-cli/callback

# One profile per patient portal account; find the practice; sign in.
modmed profiles add adult1
modmed orgs search dermatology springfield        # ModMed's directory, downloaded weekly
modmed auth login --profile adult1 --org <ID> --label adult1

# The records, as records-stream/v1 NDJSON, or filed by medrec.
modmed records export --profile adult1 --patient adult1 --ndjson > adult1.ndjson
medrec config set providers modmed                # until medrec lists modmed itself
medrec --profile <FOLDER> import --from modmed --from-profile adult1 --patient adult1 --person <WHO>
```

`auth login` opens the practice's patient portal sign-in in Chrome, in the
profile's own Chrome profile, so one account's cookies never sign in
another. Each practice is its own sign-in. `MODMED_PROFILE` stands in for
`--profile`; there is no default. `modmed info` and `modmed auth status
--profile <NAME>` say whether the app's client is stored.

### What needs a person

| Step | What the person does |
|---|---|
| Registration | Fills in the dashboard's two forms, and opens the email's verification link within 5 minutes |
| ModMed's review | Nothing; ModMed publishes no review time |
| `auth login` | Signs in on the practice's portal page with their own password and any code the portal sends, then approves the app's access |
| A new or upgraded `modmed` on macOS | Allows its first read of the vault's key in the Keychain dialog ("Always Allow") |

`records export`, `documents` and `api` need no one: modmed renews the
sign-in itself with the refresh token `offline_access` grants. ModMed does
not publish how long the tokens last; [docs/api.md](docs/api.md) records it
after the first real sign-in. When a renewal is refused, the error names
the `modmed auth login` command that signs the patient in again.

## Use

```sh
export MODMED_PROFILE=adult1                       # or --profile adult1 on each
modmed patients list                               # who is signed in, where
modmed records export --ndjson --patient adult1    # records-stream/v1 on stdout
modmed records export --ndjson --since 2026-01-01  # dated records since then
modmed documents list                              # the patient's documents (document-list/v1)
modmed documents download --all -o ~/Downloads/notes
modmed api GET 'Condition?patient=<id>'            # one read, the raw answer
```

[medrec](https://github.com/piekstra/medrec-cli) runs `modmed records
export --profile <NAME>` itself and files what it gets:

```sh
medrec --profile <FOLDER> import --from modmed --from-profile adult1 --patient adult1 --person <WHO>
```

`records export` reads the patient's record, then searches each type the
practice serves by patient, fetching each document's file right after it,
then reads the practitioners, organizations, locations and medications they
name. A search or read the practice refuses or fails, and a result a
search's answer says it left out, is listed in the stream's trailer as
skipped; the export stops only when the sign-in, the network or the output
fails. Every sign-in error ends with the `modmed` command that fixes it.

## Commands

| Command | Does |
|---|---|
| `modmed orgs refresh [--force]` | download ModMed's directories when the copy is a week old (`--force`: now) |
| `modmed orgs list [--state ST] [--limit N]`, `modmed orgs search <WORDS>` | the practices (`org-list/v1`); search: every word must match the name, id, subdomain, city, state or NPI |
| `modmed profiles list\|add <NAME>\|remove <NAME> [--force]` | the profiles, one per patient portal account (`profile-list/v1`) |
| `modmed auth set-credential [--test] [--overwrite]` | store the app's client id and secret (`--test`: a test server's) |
| `modmed auth login --profile <NAME> --org <ORG> [--label <WHO>] [--no-browser] [--timeout <MIN>]` | sign a patient in at a practice; waits up to `--timeout` minutes (default 10, at most 60) |
| `modmed auth status --profile <NAME>` | what is stored (`auth-status/v1`), per environment and patient |
| `modmed auth logout --profile <NAME> [--patient <WHO>] [--org <ORG>] [--forget]` | remove sign-ins; `--forget`, once no other profile holds one, also the client credentials and the vault's key |
| `modmed patients list --profile <NAME> [--status] [--org <ORG>]` | the profile's patients (`patient-list/v1`) |
| `modmed records export --profile <NAME> --ndjson\|-o <DIR> [--since <DATE>]` | the patient's records as `records-stream/v1`; `-o` for test servers only |
| `modmed documents list\|download --profile <NAME>` | the patient's documents (`documents/v1`) |
| `modmed api --profile <NAME> GET <PATH>` | one FHIR read as the patient |
| `modmed registration` | what to enter on ModMed's vendor dashboard to register your own app (`registration/v1`), and whether its client is stored |
| `modmed config path\|show\|set\|unset` | `redirect_port` (default 7470), `redirect_page`, `test_fhir_base`, `directory_url` |
| `modmed self-update`, `modmed completions <SHELL>`, `modmed info` | the family's standard commands |

Every command takes `--json`, except `records export --ndjson`, whose output
is already the NDJSON stream. Exit codes: 0 ok, 2 usage, 3 auth, 4 not
found, 5 upstream, 6 confirmation required.

## Build

```sh
make install    # cargo install --locked, then signs the binary (macOS)
make verify     # what CI runs
make mock       # a mock practice and directory to try it against (CONTRIBUTING.md)
```

Licensed under MIT or Apache-2.0.
