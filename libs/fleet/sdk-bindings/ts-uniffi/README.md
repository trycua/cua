# Cyclops TypeScript UniFFI bindings

Generated with `uniffi-bindgen-react-native` for Node.js N-API. The checked-in
Node.js and browser/WASM snapshots are outside
`scripts/generate-sdk-bindings.sh`; they currently expose direct record
factories, not the newer `UniffiBuilder` objects present in Rust metadata.
Browser packaging runs UBRN generation at build time, but the resulting builder
surface is not committed, drift-checked, or contract-tested. Regenerating where
applicable and validating these separate runtime and packaging pipelines is
required before builders can be advertised for either TypeScript target.

Runtime requires `@ubjs/core`, `@ubjs/node`, and a colocated
`libcyclops_sdk` cdylib. Browser source is generated in
`../ts-uniffi-browser`; packaging its WASM crate requires a project
`ubrn.config.yaml` and a wasm build pipeline.
