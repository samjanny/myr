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

## Evidence on 2026-09-25 (Linux)

- Installed: Claude Code 2.1.282 (Max subscription, `claude.ai` first-party
  auth) and Codex CLI 0.157.0 (ChatGPT login). Runs used `--subscription-only`,
  and no API key was present in the environment.
- Claude Code (`claude-sonnet-5`) completed planner, worker and reviewer calls
  in live missions. It produced structured actions of every role variant
  exercised: `submit_goal`, `define_atom`, `put_rationale`, `fetch`,
  `put_artifact`, `emit_claim`, `emit_delta`, `review_claim`, the prose-baseline
  actions and `finish`.
- Codex CLI 0.157 needed three compatibility fixes, all found before any
  billable request succeeded:
  - `features.rollout_budget.reminder_at_remaining_tokens` is now mandatory.
    Myr sets it to the empty list, so no reminder text enters the model context
    outside Myr's accounting.
  - The rollout budget is an under-development feature, and Codex reports its
    advisory as an `error` item, which the strict parser correctly rejected.
    Myr now sets `suppress_unstable_features_warning=true`.
  - An expired Codex refresh token still reported "Logged in using ChatGPT" in
    `codex login status`. The failed turn's `unauthorized (401)` is now
    reported as an authentication failure. The owner re-authenticated
    interactively.
- A replay of the recorded reviewer-B context through the corrected arguments
  returned a clean event stream with one valid Myr action.
- In live MW/0 missions Codex sometimes emitted several `agent_message` items in
  one turn. `--output-schema` constrains only the final response, and earlier
  messages are commentary. The parser now takes the message marked
  `final_answer`, or otherwise the last message not marked `commentary`.
  Commentary is never applied as an action. Two explicit final answers, or
  commentary alone, are rejected.
- The `cli_replay` example (subscription backends only) replays a request
  reconstructed from a private dispatch journal, for transport diagnosis.
- CLI failures now carry a closed diagnostic in private attempt records: the
  Claude result subtype and API status, or the Codex failure class. Free-text
  provider output is not retained. Claude's
  `error_max_structured_output_retries` is classified as INVALID_AGENT_OUTPUT
  (a structured-output failure), not provider unavailability.

The Claude smoke example requires an explicit executable and model and consumes
subscription usage when run. It is not part of automated tests. Full mission
budgets, provider compatibility across every action variant, live Codex/API
acceptance, and authoritative sandbox validation remain pending.
