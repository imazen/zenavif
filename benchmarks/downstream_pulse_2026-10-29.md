# Standalone consumer lock qualification

Linux CI run 40818658143 stopped at the existing locked downstream consumer
command: its committed lock retained two old core revisions after the manifests
moved to the optional-Pulse family. No runtime assertion failed at that step.

Starting with source f0a3919eb051cd85d204ac6592e5333b067498ef in an isolated
export, incremental `cargo metadata --manifest-path dev/downstream-probe/Cargo.toml
--format-version 1` refreshed only the requested graph. Changed package records:
ultrahdr-core, zencodec, zencodec-io, zencodec-media, zenmeta, zenpixels,
zenpixels-convert, zenravif and dependency references in zenavif, zenavif-parse
and zenavif-downstream-probe. Registry package versions/checksums were unchanged.
The old duplicate core family collapses to feb09f9f; pixels uses283655c5.

`just check-downstream`'s exact cargo command then passes with `--locked`,
including default 8/10/12-bit encoding and the unsupported SVT backend refusal.
Receipt on r7900x: rc=0,23s,peak RSS0.97GiB,minimum available13862MiB,peak load0.92.
Logs: `/home/lilith/tmp/zencodec-optional-pulse-20261029/avif-downstream01/`.
This validates the standalone default consumer, not the optional queue-audit
profile or the unresolved generic encode-orientation carrier contract.

## Standalone fuzz graphs

Both AVIF and MP4 fuzz locks were incrementally resolved from source9be62c4d
in an export without workspace overrides. AVIF changes only ultrahdr-core,
zencodec, zenmeta, zenpixels, zenpixels-convert and their consumer dependency
references. MP4 changes only zencodec, zencodec-io, zencodec-media and zenpixels.
Both contain exactly one zencodec and one zenpixels package; registry versions
and checksums are unchanged.

`cargo check --locked --manifest-path fuzz/Cargo.toml --bins` and
`just check-mp4-fuzz` pass with those locks: rc0,18s,peak RSS0.87GiB,minimum
available10427MiB,peak load0.72. Logs are in the sibling `avif-fuzz03` directory.
This compiles every configured fuzz binary; it is not a fuzz campaign or oracle
qualification. No fixture, target, feature or assertion was removed.
