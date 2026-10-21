#!/usr/bin/env node

/**
 * Build the pinned UBRN N-API runtime with a copy-based RustBuffer boundary.
 *
 * Electron 20+ rejects external ArrayBuffers. UBRN 0.31.0-3 uses them as a
 * zero-copy optimization for every RustBuffer argument and return. The
 * generated SDK already copies RustBuffer contents while lowering/lifting, so
 * this compatibility build keeps the same values and ownership while using
 * JavaScript-owned Uint8Arrays at the N-API boundary.
 *
 * UBRN 0.31.0-3 also builds a new callback trampoline, with its own
 * ThreadsafeFunction and libuv async handle, every time a JS function is passed
 * as a callback argument. The async poll loop passes its continuation callback
 * on every poll, so each await leaked one, and the host's event loop slowed as
 * it walked every leaked handle on each wakeup. UBRN fixed this in 0.31.0-4
 * (#440) by caching one trampoline per callback name and JS function. That
 * release also changes the RustBuffer boundary patched below, so this build
 * backports only the cache: node-runtime-trampoline-cache.rs is UBRN's
 * callback/cache.rs from that change, unmodified.
 *
 * The patched source remains MPL-2.0 and comes from the exact, pinned
 * uniffi-bindgen-react-native development dependency.
 */
import { spawnSync } from "node:child_process"
import {
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { basename, dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const scriptDirectory = dirname(fileURLToPath(import.meta.url))
const driverRoot = resolve(scriptDirectory, "..")
const typescriptRoot = join(driverRoot, "typescript")
const upstreamRoot = join(
  typescriptRoot,
  "node_modules",
  "uniffi-bindgen-react-native",
)
const outputIndex = process.argv.indexOf("--output")
if (outputIndex < 0 || !process.argv[outputIndex + 1]) {
  throw new Error("usage: build-node-runtime.mjs --output <path> [--target <triple>]")
}
const output = resolve(process.argv[outputIndex + 1])
const targetIndex = process.argv.indexOf("--target")
const target = targetIndex < 0 ? undefined : process.argv[targetIndex + 1]
if (targetIndex >= 0 && !target) throw new Error("missing --target value")

function cargoEnvironment() {
  const environment = { ...process.env }
  const buildsWindowsMsvc = target
    ? target.endsWith("-pc-windows-msvc")
    : process.platform === "win32"
  if (!buildsWindowsMsvc) return environment

  // Native npm packages must load on a clean Windows installation. The
  // upstream N-API runtime includes C++ objects, so MSVC's default dynamic
  // CRT linkage otherwise leaves VCRUNTIME140.dll as an undeclared system
  // prerequisite before any Cua code can run.
  environment.RUSTFLAGS = [
    environment.RUSTFLAGS?.trim(),
    "-C target-feature=+crt-static",
  ].filter(Boolean).join(" ")
  return environment
}

const manifest = JSON.parse(readFileSync(join(typescriptRoot, "package.json"), "utf8"))
const expectedVersion = manifest.devDependencies?.["uniffi-bindgen-react-native"]
const upstreamManifestPath = join(upstreamRoot, "package.json")
if (!existsSync(upstreamManifestPath)) {
  throw new Error("missing pinned UBRN source; run npm ci in libs/cua-driver/typescript")
}
const actualVersion = JSON.parse(readFileSync(upstreamManifestPath, "utf8")).version
if (actualVersion !== expectedVersion) {
  throw new Error(`UBRN source mismatch: expected ${expectedVersion}, found ${actualVersion}`)
}

function replaceOnce(source, needle, replacement, description) {
  const matches = source.split(needle).length - 1
  if (matches !== 1) throw new Error(`expected one ${description}, found ${matches}`)
  return source.replace(needle, replacement)
}

function patchRegister(source) {
  const allocStart = source.indexOf(
    "    let alloc_module = Arc::clone(&module);\n    let alloc_fn =",
  )
  const allocEndNeedle = '    result.set_named_property("rustbuffer_alloc", alloc_fn)?;'
  const allocEnd = source.indexOf(allocEndNeedle, allocStart)
  if (allocStart < 0 || allocEnd < 0) throw new Error("UBRN alloc block changed")
  source = `${source.slice(0, allocStart)}    let alloc_fn = env.create_function_from_closure("rustbuffer_alloc", move |ctx| {
        let size_arg: i32 = ctx.get(0)?;
        if size_arg < 0 {
            return Err(napi::Error::from_reason(
                "rustbuffer_alloc size must be non-negative".to_string(),
            ));
        }
        let len = usize::try_from(size_arg).map_err(|_| {
            napi::Error::from_reason("RustBuffer size exceeds addressable memory".to_string())
        })?;
        let typedarray = unsafe {
            napi_utils::create_uint8array(ctx.env.raw(), std::ptr::null(), len)?
        };
        unsafe { JsUnknown::from_raw(ctx.env.raw(), typedarray) }
    })?;
${source.slice(allocEnd)}`

  const freeStart = source.indexOf("    let free_module = Arc::clone(&module);")
  const freeEndNeedle = '    result.set_named_property("rustbuffer_free", free_fn)?;'
  const freeEnd = source.indexOf(freeEndNeedle, freeStart)
  if (freeStart < 0 || freeEnd < 0) throw new Error("UBRN free block changed")
  return `${source.slice(0, freeStart)}    let free_fn = env.create_function_from_closure("rustbuffer_free", move |ctx| {
        // Copy-mode buffers are JavaScript-owned. RustBuffer arguments are
        // separately allocated by rustbuffer_from_bytes inside call dispatch.
        ctx.env.get_undefined().map(|u| u.into_unknown())
    })?;
${source.slice(freeEnd)}`
}

function patchCall(source) {
  source = replaceOnce(
    source,
    "rust_buffer_to_js_uint8array_handoff(env, *rb, capacity_symbol)",
    "rust_buffer_to_js_uint8array_copy(env, *rb, module.rb_ops().free_ptr)",
    "RustBuffer return dispatch",
  )
  source = replaceOnce(
    source,
    "fn rust_buffer_to_js_uint8array_handoff(\n    env: &napi::Env,\n    rb: RustBufferC,\n    capacity_symbol: &CapacitySymbol,\n)",
    "fn rust_buffer_to_js_uint8array_copy(\n    env: &napi::Env,\n    rb: RustBufferC,\n    free_ptr: *const c_void,\n)",
    "RustBuffer return function",
  )
  const bodyStart = source.indexOf(
    "    // Empty RustBuffer (capacity == 0 or null data): no allocation to alias,",
  )
  const bodyEndNeedle = "    Ok(unsafe { JsUnknown::from_raw(raw_env, typedarray)? })\n}"
  const bodyEnd = source.indexOf(bodyEndNeedle, bodyStart)
  if (bodyStart < 0 || bodyEnd < 0) throw new Error("UBRN RustBuffer return body changed")
  const replacement = `    // Electron disallows external ArrayBuffers. Copy into a V8-owned
    // Uint8Array, then release the Rust allocation before returning to JS.
    let typedarray = unsafe { napi_utils::create_uint8array(raw_env, rb.data, len) };
    unsafe { napi_utils::free_rustbuffer(rb, free_ptr) };
    let typedarray = typedarray?;
    Ok(unsafe { JsUnknown::from_raw(raw_env, typedarray)? })
}`
  return `${source.slice(0, bodyStart)}${replacement}${source.slice(bodyEnd + bodyEndNeedle.length)}`
}

function patchCallbackModule(source) {
  return replaceOnce(
    source,
    "pub(crate) mod vtable;",
    "pub(crate) mod vtable;\npub(crate) mod cache;",
    "callback module list",
  )
}

function patchRegisterTrampolines(source) {
  source = replaceOnce(
    source,
    "    let capacity_symbol = Arc::new(unsafe { CapacitySymbol::new(env.raw())? });",
    `    let capacity_symbol = Arc::new(unsafe { CapacitySymbol::new(env.raw())? });
    // One cached trampoline per (callback name, JS function) for this registration.
    let trampolines = Arc::new(unsafe {
        crate::callback::cache::TrampolineCache::new(env.raw(), module.spec_callbacks().keys())?
    });`,
    "registration capacity Symbol",
  )
  source = replaceOnce(
    source,
    "        let cap_sym_for_call = Arc::clone(&capacity_symbol);",
    "        let cap_sym_for_call = Arc::clone(&capacity_symbol);\n        let trampolines_for_call = Arc::clone(&trampolines);",
    "call closure captures",
  )
  return replaceOnce(
    source,
    "                &cap_sym_for_call,\n",
    "                &cap_sym_for_call,\n                &trampolines_for_call,\n",
    "call dispatch arguments",
  )
}

function patchCallTrampolines(source) {
  source = replaceOnce(
    source,
    "    has_rust_call_status: bool,\n    capacity_symbol: &CapacitySymbol,\n) -> Result<JsUnknown> {",
    "    has_rust_call_status: bool,\n    capacity_symbol: &CapacitySymbol,\n    trampolines: &callback::cache::TrampolineCache,\n) -> Result<JsUnknown> {",
    "call dispatch signature",
  )
  return replaceOnce(
    source,
    `                let js_fn = unsafe { napi::JsFunction::from_raw(env.raw(), js_val.raw())? };
                let user_data = callback::create_callback_user_data(env, js_fn, cb_name, module)?;
                let fn_ptr = module
                    .make_callback_trampoline(
                        cb_name,
                        callback::on_js_thread,
                        callback::dispatch_to_js_thread,
                        callback::is_js_thread,
                        user_data,
                    )
                    .map_err(core_err)?;
                slot::write_pointer(slot, fn_ptr);`,
    `                // Trampolines are never freed, so build at most one per JS function.
                let raw_fn_val = unsafe { js_val.raw() };
                let cached = unsafe { trampolines.get(env.raw(), raw_fn_val, cb_name)? };
                let fn_ptr = match cached {
                    Some(fn_ptr) => fn_ptr,
                    None => {
                        let js_fn = unsafe { napi::JsFunction::from_raw(env.raw(), raw_fn_val)? };
                        let user_data =
                            callback::create_callback_user_data(env, js_fn, cb_name, module)?;
                        let fn_ptr = module
                            .make_callback_trampoline(
                                cb_name,
                                callback::on_js_thread,
                                callback::dispatch_to_js_thread,
                                callback::is_js_thread,
                                user_data,
                            )
                            .map_err(core_err)?;
                        unsafe { trampolines.set(env.raw(), raw_fn_val, cb_name, fn_ptr)? };
                        fn_ptr
                    }
                };
                slot::write_pointer(slot, fn_ptr);`,
    "callback argument dispatch",
  )
}

const temporaryRoot = mkdtempSync(join(tmpdir(), "cua-driver-node-runtime-"))
try {
  const runtimeRoot = join(temporaryRoot, "runtime")
  cpSync(join(upstreamRoot, "runtimes", "core"), join(runtimeRoot, "core"), {
    recursive: true,
  })
  cpSync(join(upstreamRoot, "runtimes", "napi"), join(runtimeRoot, "napi"), {
    recursive: true,
  })
  const registerPath = join(runtimeRoot, "napi", "src", "register", "mod.rs")
  writeFileSync(registerPath, patchRegister(readFileSync(registerPath, "utf8")))
  const callPath = join(runtimeRoot, "napi", "src", "call", "mod.rs")
  writeFileSync(callPath, patchCallTrampolines(patchCall(readFileSync(callPath, "utf8"))))
  writeFileSync(registerPath, patchRegisterTrampolines(readFileSync(registerPath, "utf8")))
  const callbackPath = join(runtimeRoot, "napi", "src", "callback", "mod.rs")
  writeFileSync(callbackPath, patchCallbackModule(readFileSync(callbackPath, "utf8")))
  copyFileSync(
    join(scriptDirectory, "node-runtime-trampoline-cache.rs"),
    join(runtimeRoot, "napi", "src", "callback", "cache.rs"),
  )

  const args = ["build", "--release", "--manifest-path", join(runtimeRoot, "napi", "Cargo.toml")]
  if (target) args.push("--target", target)
  const result = spawnSync("cargo", args, {
    env: cargoEnvironment(),
    stdio: "inherit",
  })
  if (result.error) throw result.error
  if (result.status !== 0) throw new Error(`cargo exited with status ${result.status}`)

  const targetDirectory = target
    ? join(runtimeRoot, "napi", "target", target, "release")
    : join(runtimeRoot, "napi", "target", "release")
  const candidates = [
    "libuniffi_runtime_napi.dylib",
    "libuniffi_runtime_napi.so",
    "uniffi_runtime_napi.dll",
  ]
  const built = candidates.map((name) => join(targetDirectory, name)).find(existsSync)
  if (!built) throw new Error(`missing built N-API runtime under ${targetDirectory}`)
  mkdirSync(dirname(output), { recursive: true })
  copyFileSync(built, output)
  console.log(`built ${basename(output)} from UBRN ${actualVersion}`)
} finally {
  rmSync(temporaryRoot, { recursive: true, force: true })
}
