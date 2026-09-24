# Sandbox execution status

Verifier policies declare network denial, memory and process limits, exact argv,
environment and tool versions. Candidate preparation and an explicit Docker Linux
executor are implemented. Live confinement conformance is not yet verified.
Running an ordinary child process must not be labeled deterministic sandbox evidence.

The shared `myr_adapter::process::run_bounded` primitive supports per-stream and
combined output caps up to 64 MiB, a single deadline covering stdin, both output
pipes and process exit, and process-tree termination on failure. Caps are checked
as chunks arrive, including a combined cap when the child keeps its pipes open.
Successful collection retains `exit_code: Option<i32>`; a normal nonzero exit is
different from timeout, capture failure or termination without a code. Existing
provider transports use their original 2 MiB per-stream default. This primitive
is not a security boundary, and its provider-oriented environment allowlist is
not a verifier environment policy.

On 2026-09-24, read-only probes found Docker client 29.7.2 but no accessible local
daemon pipe; the WSL distribution listing was empty. These are observations from
the current execution session, not proof of system-wide absence. No system
configuration was changed and no images were downloaded.

A container backend must be explicitly selected and sealed, with pinned
images and verified confinement. Docker's documented controls include network
isolation, resource limits, read-only root filesystems and privilege restrictions:
[container execution reference](https://docs.docker.com/engine/containers/run/),
[none network](https://docs.docker.com/engine/network/drivers/none/).
Their presence in command arguments alone does not establish runtime conformance.
Existing declared native backends remain Linux Bubblewrap and Windows AppContainer,
both pending implementation. Neither falls back to Docker.

## Explicit Docker execution

The sealed backend shape is `{"docker_linux":{"image":"sha256:<64 lowercase hex digits>"}}`.
Only a local image ID is accepted, and creation uses `--pull=never`. The operator
supplies an absolute Docker executable. The executor uses an empty temporary CLI
configuration, clears inherited routing/authentication variables, and connects only
to the default local daemon socket/pipe. Remote contexts and rootless custom sockets
are not currently supported. The image must provide `/usr/bin/env`, `/bin/sh`,
`/bin/cp`, and the declared verifier tools. Image choice is trusted operator input.

The executor reopens the candidate manifest and seal, requires the policy to be
sealed, and rereads candidate bytes before creating the container. It checks daemon
memory/swap/PID cgroup support and default seccomp, verifies the image identity and
rejects image-declared volumes. The input tree is mounted read-only at `/input`;
a fixed bootstrap copies it into a bounded `/work` tmpfs. A separate bounded `/tmp`
tmpfs is available. Host worktrees, credentials and CAS are not mounted. Network is
`none`, capabilities are dropped, new privileges are disabled, the root filesystem
is read-only, restart/healthcheck/logging are disabled, and memory/swap/PID caps apply.
Unix execution uses the input directory owner's numeric UID/GID; Windows uses the
container root UID without Linux capabilities.

Before start, inspection must match the requested image, command, user, mounts,
isolation controls and limits. After attached execution, inspection must establish
normal termination, no OOM, and an exit code matching the attached CLI result.
Ordinary nonzero command exits are retained. Codes 125–127 are conservatively
classified as bootstrap/startup infrastructure failures. The policy deadline
covers Docker metadata, creation, execution and post-inspection; cleanup receives
an additional bounded ten seconds. Failed cleanup reports the container name and
never returns a successful receipt. Abrupt runtime termination and daemon outages
can still require manual container cleanup; crash recovery remains pending.
Cleanup first inspects an execution-specific ownership label, then removes the
verified container ID. A name collision with an unrelated container does not
authorize its removal; ownership mismatch is covered by a no-delete test.

The returned receipt contains candidate/policy references, outputs, exit code and
Docker metadata. This low-level executor does not emit EVIDENCE or promote FACTs;
the separate [command verification runtime](command-evidence.md) performs recording
and semantic insertion. Tool-version strings
are sealed declarations, not fresh executable version probes. The caller still
needs live confinement acceptance, mission deadline integration and complete
terminal orchestration. Candidate rereading currently uses snapshot reader default size limits.

Unit tests inject Docker responses to cover literal argv, policy selection,
inspection mismatches, nonzero exits, OOM, timeout and cleanup failures. These tests
do not execute containers and do not prove kernel confinement or Docker compatibility.
The default daemon was unavailable in the last live probe, so live positive and
adversarial acceptance remains mandatory before claiming sandbox conformance.
