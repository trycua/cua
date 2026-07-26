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
  writeFileSync(callPath, patchCall(readFileSync(callPath, "utf8")))

  const args = ["build", "--release", "--manifest-path", join(runtimeRoot, "napi", "Cargo.toml")]
  if (target) args.push("--target", target)
  const result = spawnSync("cargo", args, { stdio: "inherit" })
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
