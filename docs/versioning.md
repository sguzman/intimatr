# Versioning and compatibility

Intimatr has three version surfaces that should not be conflated: the Rust package/release version, the RPC protocol version, and the persisted analysis-workspace format version.

## Package and release version

`Cargo.toml` is the source of truth for the package version. Release tags use `v<package-version>`; for example, package version `0.1.0` is released from tag `v0.1.0`.

Because Intimatr is shipped as an executable DLL/tool rather than only as a reusable Rust library, the repository also commits `Cargo.lock`. Release and CI builds therefore use one reviewed dependency graph instead of silently resolving a different compatible graph on a later day. Updating dependencies is an explicit repository change that can be tested and benchmarked before release.

The release workflow rejects a tag whose version does not match `Cargo.toml`. A successful tagged release builds the Windows x86_64 DLL, validates the source, packages the example configuration and protocol/extension documentation, uploads the ZIP as a workflow artifact, and attaches the same ZIP to the GitHub release.

## RPC protocol version

`src/rpc/protocol.rs` defines `PROTOCOL_VERSION`. The current protocol version is **1**.

Protocol v1 consists of:

- a four-byte unsigned big-endian frame length;
- a JSON request/response envelope;
- a request ID used to correlate responses;
- an explicit protocol-version field;
- serialized `Command`/`CommandResult` payloads.

Adding a new command/result variant does not by itself require a framing-version bump when old clients can continue decoding the envelope they understand. Change `PROTOCOL_VERSION` when framing, envelope semantics, or another incompatible transport-level contract changes. Servers reject unsupported versions explicitly instead of guessing compatibility.

## Analysis workspace version

`src/analysis.rs` defines `WORKSPACE_VERSION`. The current workspace format version is **1**.

Increase the workspace version before making an incompatible change to persisted scan/watch-template meaning or JSON structure. Loading must fail clearly on an unsupported format rather than silently interpreting state under new semantics.

Saved scalar scans contain process-address snapshots and are not promised to survive process-layout changes. Module-relative watch templates are the preferred reusable cross-run representation when a stable module offset exists.

## Release compatibility notes

Each packaged release includes a `VERSION.txt` summary containing package version, platform, RPC protocol version, workspace version, and project scope, plus a SHA-256 checksum for the packaged DLL. Detailed command semantics live in `docs/rpc.md`; analysis persistence semantics live in `docs/analysis.md`; architectural extension constraints live in `docs/extensions.md`.

Until the package reaches a declared stable compatibility policy, consumers should pin the Intimatr package/release version as well as checking the protocol/workspace versions they depend on. Version fields exist so incompatibility is explicit rather than accidental.
