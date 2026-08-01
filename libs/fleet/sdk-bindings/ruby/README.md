# Ruby Contract Binding

Run from any working directory after setting `REPO_ROOT` to the absolute path of
this clone (for example, `REPO_ROOT=/path/to/clone`):

```bash
REPO_ROOT=/path/to/clone
$REPO_ROOT/cyclops-cs/scripts/run-ruby-sdk-binding.sh "$REPO_ROOT/cyclops-cs/sdk-bindings/ruby/tests/test_async_client.rb"
$REPO_ROOT/cyclops-cs/scripts/run-ruby-sdk-binding.sh "$REPO_ROOT/cyclops-cs/sdk-bindings/examples/ruby/app_controlled.rb"
```

The launcher stages the Ruby files and native library in a temporary load path, preserving the generated source tree.
