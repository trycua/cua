# Cyclops Go UniFFI bindings

Generated with `uniffi-bindgen-go v0.7.1+v0.31.0` from the Cyclops Rust cdylib.
The checked-in output includes the schema records and generated builder objects
exposed by Rust metadata.

Regenerate all canonical Fleet SDK bindings from the repository root with:

```sh
./libs/fleet/scripts/generate-sdk-bindings.sh
./libs/fleet/scripts/generate-sdk-bindings.sh --check
```

The script requires `uniffi-bindgen-go` version
`uniffi-bindgen 0.7.1+v0.31.0` and records the complete generated file set in
`.cyclops-sdk-generated-files`.

Build consumers with the Cyclops Rust cdylib available to cgo, for example:

```sh
CGO_LDFLAGS='-L/path/to -lcyclops_sdk' LD_LIBRARY_PATH=/path/to go test ./...
```
