# Mission YAML input

`myr check-mission <file>` validates the initial YAML mission and prints its
normalized goal, verifier argv arrays, and protected paths. It does not create
a store, call a planner, seal a goal, or execute a verifier.

```yaml
goal: >
  Replace the cache implementation while preserving existing behavior.
verify:
  - cargo test
  - cargo clippy -- -D warnings
protected:
  - tests/
  - myr.yaml
```

Each verifier may instead be an explicit array, for example
`['C:\Program Files\tool.exe', '--input', 'file with spaces']`. This form preserves
every argument literally, including empty arguments and shell metacharacters.
String shorthand uses shell-style word quoting only: no shell process, variable
expansion, pipeline or redirection is implied. Shorthand containing shell control
characters is rejected, even inside quotes; use argv arrays for those literals.
Use arrays for Windows paths to avoid shorthand backslash interpretation.

The parser requires one document, a nonempty goal and at least one verifier. It
rejects unknown/duplicate fields, malformed quoting, NUL arguments, paths outside
the repository, case-colliding protected paths and files over 1 MiB. Protected
directory suffixes are normalized and the path set is sorted.

Parsing uses pinned `serde_yaml_ng` 0.10.0 and `shlex` 2.0.1. The YAML API is
documented by [serde_yaml_ng](https://docs.rs/serde_yaml_ng/0.10.0/serde_yaml_ng/).
Planner invocation, baseline import, observed tool-version capture, verifier
policy construction remain separate integration work; a successful syntax check
is not a successful mission.

`myr seal --mission myr.yaml --ir goal-ir.json --policy seal-policy.json --root .myr`
binds an existing planner IR proposal to the parsed user mission before sealing.
The goal text must match exactly. Every requested verifier must occur as a positive,
binding `core.command_succeeds` criterion naming a sealed command policy with the
same argv and repository-root cwd. Repeated required commands need distinct policy
references; one criterion cannot silently satisfy multiple requested executions.
User-protected paths are merged into the policy even if omitted by the proposal.
Extra planner criteria still pass the normal registry/class/reference validation.
This command requires referenced objects already in the store; it does not yet
invoke the planner or construct those objects from a repository automatically.
