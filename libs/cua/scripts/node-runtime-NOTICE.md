# cua SDK Node runtime notice

`cua_node_runtime.node` is a compatibility build derived from the N-API
runtime in `uniffi-bindgen-react-native` 0.31.0-3, copyright its contributors
and licensed under the Mozilla Public License 2.0.

The corresponding source is the pinned npm development dependency plus the
deterministic transformations in `scripts/build-node-runtime.mjs`. Those
transformations add `scripts/node-runtime-trampoline-cache.rs`, which is
`runtimes/napi/src/callback/cache.rs` from `uniffi-bindgen-react-native` commit
5d24b38 (jhugman/uniffi-bindgen-react-native#440, released in 0.31.0-4),
unmodified and under the same license. The source and build script are
available in the Cua repository at the release tag that matches this package.

<https://www.mozilla.org/MPL/2.0/>
