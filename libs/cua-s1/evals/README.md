# Offline evaluation scaffold

`metrics.evaluate_predictions` compares normalized JSON-like records from any
model or provider. It never performs inference or network access.

Gold records require a stable `id`, `action`, and optional `target`. Ambiguous
cases may instead include an `acceptable` list:

```json
{"id":"city","acceptable":[{"action":"fill","target":2},{"action":"fill","target":5}]}
```

Prediction records use the same `id`, `action`, and optional `target` shape.
Missing predictions are scored as abstentions. The report separates exact
accuracy, coverage, selective accuracy, wrong actions, wrong targets, and
actions taken where the gold behavior required abstention.
