# Python Contract Binding

Run from any working directory after setting `REPO_ROOT` to the absolute path of
this clone (for example, `REPO_ROOT=/path/to/clone`):

```bash
REPO_ROOT=/path/to/clone
$REPO_ROOT/cyclops-cs/scripts/run-python-sdk-binding.sh "$REPO_ROOT/cyclops-cs/sdk-bindings/python/tests/test_async_client.py" -v
$REPO_ROOT/cyclops-cs/scripts/run-python-sdk-binding.sh "$REPO_ROOT/cyclops-cs/sdk-bindings/examples/python/app_controlled.py"
```

The launcher builds the cdylib once in `cyclops-cs/target/sdk-bindings-native`, stages a temporary Python package, and deletes it on exit. It never copies a native library into generated source.
