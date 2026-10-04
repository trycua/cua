/**
 * Post-processing of the UniFFI Python binding (src/cua/_native.py).
 *
 * UniFFI's object template frees the Rust handle in `__del__` through the
 * module globals `_uniffi_rust_call` and `_UniffiLib`. At interpreter exit
 * CPython clears module globals before the last objects are collected, so
 * every live handle printed
 * `Exception ignored in: <function Cua.__del__> ... AttributeError: 'NoneType'
 * object has no attribute 'uniffi_cua_sdk_fn_free_cua'`.
 *
 * The rewrite binds the free function and the call helper as default
 * arguments (evaluated once, at class creation) and skips the free while the
 * interpreter is finalizing: the process is exiting and the OS reclaims the
 * handle, and calling into Rust while Python tears down is not safe.
 *
 * Every `__del__` of the generated module must match the template; a UniFFI
 * upgrade that changes it fails generation instead of silently regressing.
 */

const FINALIZER =
  /( {4})def __del__\(self\):\n\1 {4}# In case of partial initialization of instances\.\n\1 {4}handle = getattr\(self, "_handle", None\)\n\1 {4}if handle is not None:\n\1 {8}_uniffi_rust_call\(_UniffiLib\.(uniffi_[A-Za-z0-9_]+_fn_free_[A-Za-z0-9_]+), handle\)\n/g

export function postprocessPython(source) {
  const total = source.split("def __del__(").length - 1
  let rewritten = 0
  const output = source.replace(FINALIZER, (_match, indent, free) => {
    rewritten += 1
    const body = `${indent}    `
    return [
      `${indent}def __del__(`,
      `${body}    self,`,
      `${body}    _uniffi_free=_UniffiLib.${free},`,
      `${body}    _uniffi_call=_uniffi_rust_call,`,
      `${body}    _uniffi_finalizing=sys.is_finalizing,`,
      `${indent}):`,
      `${body}# In case of partial initialization of instances.`,
      `${body}handle = getattr(self, "_handle", None)`,
      `${body}# At interpreter exit the module globals are gone: skip the free (cua`,
      `${body}# post-process, libs/cua/scripts/uniffi-python-postprocess.mjs).`,
      `${body}if handle is not None and not _uniffi_finalizing():`,
      `${body}    _uniffi_call(_uniffi_free, handle)`,
      "",
    ].join("\n")
  })
  if (total === 0 || rewritten !== total) {
    throw new Error(
      `UniFFI Python finalizers changed shape: rewrote ${rewritten} of ${total} __del__ methods; ` +
        "update libs/cua/scripts/uniffi-python-postprocess.mjs",
    )
  }
  if (!/^import sys$/m.test(output)) {
    throw new Error("the generated Python binding no longer imports sys")
  }
  return output
}
