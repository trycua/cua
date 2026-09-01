# Cua Sandbox Runtime Inspection

`scripts/docs-generators/cua_sandbox_runtime.py` generates the sandbox API page
from Python's `inspect` and `typing` APIs. It imports the package, uses
`cua_sandbox.__all__` as the sole public boundary, and follows public type hints
reachable from those exports. The public interface classes loaded by the exported
`Sandbox` are taken from `cua_sandbox.interfaces.__all__`. It does not parse Python source or use Griffe,
pdoc, Sphinx, or AST tooling.

## Controlled import

Runtime inspection executes import-time code. Run it only from a trusted checkout
in a disposable virtual environment. The generator clears caller configuration
before importing, disables user site packages and bytecode writes, and sets
`CUA_SANDBOX_DOCS_RUNTIME_INSPECTION=1`. It does not make network requests
itself, but imported dependencies remain responsible for their import behavior.

The `cua-sandbox` package's Hatch build hook invokes Cargo to build the generated
Fleet/Cyclops binding. The public documentation explicitly excludes Fleet and
`cyclops_sdk`, so the documented command installs the pinned dependencies used by
the imported public surface, adds the sandbox source tree to `sys.path`, and
supplies an in-memory stub only for the excluded `cyclops_sdk` import. It does not
install `cua-auto`: that optional runtime is not imported by this reference and
its Linux `evdev` dependency requires native Python headers. Use
`--no-excluded-dependency-stub` in an environment with the real binding to verify
the full import path.

Static candidates can analyze source without executing imports, but can miss the
resolved runtime export list, descriptor signatures, coroutine behavior, and
conditional imports. This generator deliberately accepts the trusted-checkout
import tradeoff, while constraining that import to the public surface and the
minimal pinned helper environment above.

```bash
uv venv .venv-cua-957-runtime --python 3.11
uv pip install --python .venv-cua-957-runtime/bin/python \
  --requirements scripts/docs-generators/cua_sandbox_runtime_requirements.txt
.venv-cua-957-runtime/bin/python scripts/docs-generators/cua_sandbox_runtime.py
.venv-cua-957-runtime/bin/python scripts/docs-generators/cua_sandbox_runtime.py --check
```

The checked-in page is byte-compared by `--check`. Generation is deterministic:
it preserves `__all__` ordering for public exports, sorts discovered reachable
interfaces by name, normalizes line endings, and includes no timestamp, absolute
path, environment value, or dependency version in the output.
