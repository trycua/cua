# Component map

## Benchmark definition

`benchmarks/cua-driver-bench/` owns the public benchmark definition, profiles,
calibration method, local comparison runner, documentation, and task-pack
interface. It contains no held-out task payloads.

## Runtime

`libs/cua-bench-runtime/` owns the `cua-bench-runtime` distribution,
`cua_bench_runtime` import package, bundled schemas, synthetic conformance suite,
and the `cdb` executable. It remains separate from `libs/cua-bench`, whose `cb`
executable serves the broader Cua benchmark toolkit.

## Held-out task pack

The separately controlled task pack owns task manifests, participant briefs,
fixtures, reset logic, evaluators, expected state, and task evidence. Callers
provide its root explicitly. The runtime never discovers it implicitly from the
monorepo.

## Dependency direction

```text
benchmark definition + authorized task pack
                    |
                    v
            cua-bench-runtime
                    |
                    v
       trial directories and reports
```

The automated-evaluation runner orchestrates the runtime and explicit local Cua
Driver releases. Its results are diagnostic and never certifying.
