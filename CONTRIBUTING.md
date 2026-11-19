# Contributing

- Read `AGENTS.md` (layout, rules for medical data and credentials) and
  `docs/api.md` (ModMed's API and terms, their sources and traps) first.
- `make verify` runs what CI runs: fmt, clippy with `-D warnings`, the tests
  and the smoke test on the release binary. It needs no network beyond the
  first dependency fetch and never reads the real Keychain.
- ModMed has no sandbox for this API. Dogfood the installed binary against
  `make mock`: medical-common's mock SMART + FHIR server serving one
  synthetic patient as the `test` organization, and a stand-in for ModMed's
  directory serving synthetic practices. It prints both addresses and the
  synthetic client pair:

  ```sh
  make install && make mock     # in another terminal
  export MODMED_KEYCHAIN_SERVICE=piekstra.modmed.dogfood MODMED_PROFILE=dogfood
  export MODMED_CACHE_DIR=/tmp/modmed-dogfood-cache
  modmed config set test_fhir_base <the mock's FHIR base>
  modmed config set directory_url '<the mock's directory>'
  modmed orgs list
  modmed profiles add dogfood
  printf '%s' '<the printed pair>' | modmed auth set-credential --test --stdin
  modmed auth login --org test --no-browser   # then open the printed address
  modmed records export -o /tmp/modmed-mock
  modmed documents download --all -o /tmp/modmed-mock-docs
  modmed auth logout --forget && modmed profiles remove dogfood --force
  ```

  With `MODMED_KEYCHAIN_SERVICE` set, the vault lives beside the config
  (`vault.<service>/`), and `auth logout --forget` removes its key item
  once nothing is left in it. The directory's practices are invented: a
  sign-in at one goes to ModMed's servers, which do not know it.

- Test data is invented. Never commit a real response, record, export or
  directory page; `tests/no_personal_data.rs`, `tests/fixture_shapes.rs`
  and gitleaks check every change. The client id and secret never appear in
  the repository: ModMed's terms forbid credentials in open source
  projects.
- Every change lands through a pull request with a review and green checks.
- Dates on the command line are `YYYY-MM-DD`. Every command takes
  `--json`, except `records export --ndjson`, whose stream is the output.
