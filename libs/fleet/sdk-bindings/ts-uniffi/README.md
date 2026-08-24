# Cyclops TypeScript UniFFI bindings

Generated with `uniffi-bindgen-react-native 0.31.0-3` for Node.js N-API. The
checked-in Node.js and browser/WASM snapshots include the schema records and
generated builder objects exposed by Rust metadata.

From the Fleet workspace, regenerate Node.js bindings with:

```sh
cargo build --locked --release -p cyclops-sdk
npx --yes --package uniffi-bindgen-react-native@0.31.0-3 ubrn \
  generate napi bindings target/release/libcyclops_sdk.so --library \
  --ts-dir sdk-bindings/ts-uniffi --lib-colocated
```

Regenerate the checked-in browser bridge and TypeScript modules with:

```sh
npx --yes --package uniffi-bindgen-react-native@0.31.0-3 ubrn \
  generate wasm bindings target/release/libcyclops_sdk.so --library \
  --ts-dir sdk-bindings/ts-uniffi-browser/ts \
  --cpp-dir sdk-bindings/ts-uniffi-browser/cpp
```

Runtime requires `@ubjs/core`, `@ubjs/node`, and a colocated `libcyclops_sdk`
cdylib. Browser packaging uses `ts-uniffi-browser/ubrn.config.yaml` and its WASM
build pipeline.
