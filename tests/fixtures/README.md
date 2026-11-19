# Test fixtures

Invented payloads. Nothing here was captured from ModMed or describes a real
practice or person; every value is a dummy.

| File | Stands for |
|---|---|
| `directory/ema-page1.json` | The first page of ModMed's endpoint directory (`Endpoint?_include=Endpoint:managingOrganization&connection-type=hl7-fhir-rest`), shaped as ModMed serves it: each `Endpoint` beside its managing `Organization`, a `next` link, `"search": null` entries. It carries the cases the parser handles: a practice with an NPI and a subdomain identifier, one listed twice at one address (the second with `Organization/1.null`), an endpoint that is `off`, and one whose address is not ModMed's |
| `directory/ema-page2.json` | The last page: no `next` link, an organization with no address |

Rules `tests/fixture_shapes.rs` checks:

- Practice names use `Example`, `Sample`, `Demo`, `Closed` or `Elsewhere`.
- An NPI starts with `1555`; a phone number with `555-01`.
- A street address is `<n> Example St`, a postal code `00000`, a state `ST`.
- Ids are `1.5550NN…`, and hosts are invented subdomains of ModMed's
  `fhir.ema-api.com` or `elsewhere.example`.

The FHIR records `tests/e2e.rs` reads are medical-common's synthetic
patient, served by its mock server (`pk_med_fhir::test_util`).
