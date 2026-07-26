# Cyclops Go UniFFI bindings

Generated with `uniffi-bindgen-go v0.7.1+v0.31.0`. Build consumers with the
Cyclops Rust cdylib available to cgo, for example:

`CGO_LDFLAGS='-L/path/to -lcyclops_sdk' LD_LIBRARY_PATH=/path/to go test ./...`
