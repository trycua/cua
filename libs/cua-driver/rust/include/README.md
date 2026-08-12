# Cua Driver native ABI

`cua_driver_abi.h` is the stable native interoperability boundary below the
public Cua Driver SDK. Release archives place it beside the platform shared
library.

The ABI uses versioned symbols, opaque handles, explicit buffer ownership, and
callbacks for asynchronous work. Call `cua_driver_abi_version_v1` and
`cua_driver_abi_is_compatible_v1` before creating a driver. Every returned
`CuaDriverBuffer` must be released with `cua_driver_buffer_free_v1`; driver and
operation release functions accept pointer-to-pointer values and are
idempotent after clearing them.

Applications should normally use the safe Rust, Python, or TypeScript SDK.
The C ABI exists for native embedding and to keep those SDKs independent of the
private language used to implement the core.
