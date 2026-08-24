# Cyclops Go UniFFI bindings

Generated with `uniffi-bindgen-go v0.7.1+v0.31.0` from the Cyclops Rust cdylib.
The checked-in snapshot includes the schema records and generated builder
objects exposed by Rust metadata.

From the Fleet workspace, regenerate with:

```sh
cargo build --locked --release -p cyclops-sdk
uniffi-bindgen-go target/release/libcyclops_sdk.so --library \
  --out-dir sdk-bindings/go-uniffi
```

Build consumers with the Cyclops Rust cdylib available to cgo, for example:

```sh
CGO_LDFLAGS='-L/path/to -lcyclops_sdk' LD_LIBRARY_PATH=/path/to go test ./...
```
