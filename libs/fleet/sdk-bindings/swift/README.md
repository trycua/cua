# Swift Contract Binding

On macOS with Xcode Swift installed, set `REPO_ROOT` to the absolute path of this
clone (for example, `REPO_ROOT=/path/to/clone`), then compile generated schema
and SDK sources into one `CyclopsSdk` module and execute the contract test:

```bash
REPO_ROOT=/path/to/clone
$REPO_ROOT/cyclops-cs/scripts/run-swift-sdk-binding.sh "$REPO_ROOT/cyclops-cs/sdk-bindings/swift/tests/TestAsyncClient.swift"
$REPO_ROOT/cyclops-cs/scripts/run-swift-sdk-binding.sh "$REPO_ROOT/cyclops-cs/sdk-bindings/examples/swift/AppControlled.swift"
```

The launcher passes both generated Swift source files to one `swiftc` invocation and uses the checked-in FFI module maps plus an rpath to the freshly-built cdylib.
