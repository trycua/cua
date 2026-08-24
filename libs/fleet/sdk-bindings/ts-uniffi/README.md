# Cyclops TypeScript UniFFI bindings

Generated with `uniffi-bindgen-react-native 0.31.0-3` for Node.js N-API. The
checked-in Node.js and browser/WASM outputs include the schema records and
generated builder objects exposed by Rust metadata.

Regenerate and verify all canonical Fleet SDK bindings from the repository root
with:

```sh
./libs/fleet/scripts/generate-sdk-bindings.sh
./libs/fleet/scripts/generate-sdk-bindings.sh --check
```

The canonical script invokes the pinned package for both Node.js and browser
targets and records each complete generated file set in
`.cyclops-sdk-generated-files`.

Runtime requires `@ubjs/core`, `@ubjs/node`, and a colocated `libcyclops_sdk`
cdylib. Browser packaging uses `ts-uniffi-browser/ubrn.config.yaml` and its WASM
build pipeline.
