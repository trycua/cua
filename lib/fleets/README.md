# Vendored Cyclops Python SDK

This directory vendors the generated `cyclops_sdk` Python bindings from
`trycua/cloud` PR #5905, commit `239a06064`. It also contains the upstream
contract fixture and tests used to verify the UniFFI callback contract.

The bindings require the native `libcyclops_sdk` cdylib from `libs/fleet/sdk`.
Build it with `cargo build -p cyclops-sdk`, then place the platform library next
to `cyclops_sdk/_sdk.py` and `cyclops_sdk/_schema.py` before importing the package.
