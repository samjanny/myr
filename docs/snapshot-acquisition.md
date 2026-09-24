# Repository snapshot acquisition

`myr snapshot <repository> --root <new-store>` reads repository bytes and imports
the resulting baseline manifest and files into CAS/SQLite. It returns snapshot,
file and provenance references as JSON. Existing output directories are rejected;
the source directory is not modified and no command or model is invoked.

Default `ReadPolicy` excludes root `.git`, `.myr` and `target` subtrees, with
case-insensitive portable prefix matching. These are explicit policy exclusions,
not inferred from `.gitignore`. Limits are 100,000 directory entries, 16 MiB per
file, 256 MiB total and depth 128. `--read-policy policy.json` accepts a complete
replacement policy; the applied policy is retained in a provenance artifact.
Exceeding a limit fails the snapshot rather than silently truncating files.

The reader preserves opaque bytes, validates portable paths, rejects symlinks,
Windows reparse points and nonregular files, and compares file size and modified
time around each bounded read. Acquisition runs before workers start. It is not
an OS sandbox or an atomic snapshot against hostile concurrent filesystem
changes; the orchestration layer must acquire a stable input checkout. Source-view
benchmark poison must never be imported into the shared worker store.

Windows tests cover deterministic reads, binary bytes, exclusions, resource
limits, invalid exclusion paths and public CLI import/no-overwrite behavior. A
Unix-only symlink test exists but has not been run in this Windows environment;
Windows reparse-point rejection still needs an actual junction/symlink test.
