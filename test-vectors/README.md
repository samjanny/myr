# MW/0 reference vectors

`mw0-v0.json` contains canonical JSON input, exact CBOR bytes, and the expected
domain-separated BLAKE3 CID for all ten message kinds. These are regression
vectors produced by the authoritative Rust implementation, not a claim of
independent cross-implementation certification. The CLAIM layout is also
checked against hand-assembled RFC 8949 bytes.

The reference CIDs use dummy referenced CIDs; wire validity does not imply that
their dependencies exist in a graph. Integration tests construct real graphs.

To propose a deliberate protocol revision, export candidate vectors with:

```text
cargo run -p myr-wire --example export_vectors --offline
```

Do not regenerate expected vectors automatically when tests fail. Normative
freeze is pending the adapter/runner integration and independent CDDL check.
