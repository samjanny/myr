# Provider transports

`myr_adapter::transport::complete` dispatches an explicit `ProviderConfig` to
Claude Code, Codex CLI, OpenAI Responses, or Anthropic Messages. It does not retry
with another backend. Returned bytes remain untrusted until `Session::handle`
performs role, schema, reference, and capability validation.

API transports use fixed HTTPS origins, disable redirects, read only the named
API-key environment variable, and bound response size and request duration.
Local HTTP mocks check both request authentication and response parsing. There
have been no live paid-API requests.

CLI transports clear inherited environment variables except an explicit OS/auth
location allowlist, use temporary working directories, and bound process output
and elapsed time. Supported CLI diagnostics check subscription authentication;
Myr neither reads credential files nor extracts OAuth tokens. Native tools,
customizations, MCP, and persistent conversations are disabled where supported.
Codex additionally requests read-only filesystem confinement and disabled network
access for model tools. These settings require live sandbox-conformance checks;
post-response rejection of native-tool events alone is not a security sandbox.

## Evidence on 2026-09-24

- Workspace: 50 passing tests, including HTTP mocks, process timeouts/output
  limits, authentication rejection, CLI arguments, structured response parsing,
  and the existing semantic validation suite. Clippy passes with warnings denied.
- Installed Claude Code: 2.1.281. Supported authentication diagnostics confirmed
  Claude.ai, first-party routing, and a Max subscription.
- One authorized live Claude Code call completed successfully using
  `claude-haiku-4-5`. The prompt requested only `finish`; the output parsed as
  `{"action":{"tool":"finish","arguments":{}}}`. Reported usage: 2,992 input,
  226 output, zero cached-input tokens. No project files were supplied. This is
  transport evidence, not a full mission or benchmark result.
- An earlier sandboxed attempt timed out at 60 seconds; its provider usage is
  unknown. An initial elevated retry was rejected by automatic approval review.
  The owner then explicitly authorized the minimal subscription call above.
- Codex CLI 0.156.1: read-only help/features inspected; live completion and
  confinement have not been verified. No Codex model call was made.
- Declared Rust minimum is 1.88 because of resolved dependency requirements;
  tests ran with Rust 1.95.0 on Windows. Minimum-version and Linux checks remain.

The Claude smoke example requires an explicit executable and model and consumes
subscription usage when run. It is not part of automated tests. Full mission
budgets, provider compatibility across every action variant, live Codex/API
acceptance, and authoritative sandbox validation remain pending.
