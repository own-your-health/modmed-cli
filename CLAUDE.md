# CLAUDE.md

`AGENTS.md` is canonical; read it first. The rules that matter most:

- Never read, print or summarize a real record, document or patient name.
  ModMed has no sandbox: develop and dogfood on the mock server (`make
  mock`, the `test` organization) only.
- Never print or log a token, client id or secret, never pass one in argv,
  and never commit one (ModMed's terms forbid credentials in open source
  projects).
- If a `modmed` call hangs, it waits on a Keychain approval: stop calling it
  and tell the owner.
- No MCP surface: no `mcp` command and no cli-mcp registration.
