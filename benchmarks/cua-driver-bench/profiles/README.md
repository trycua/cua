# Experiment profiles

Profiles declare harness and driver conditions without changing task content.
Version 0.1 defines exactly five profile IDs.

| Profile ID | Requirement | Condition |
| --- | --- | --- |
| `native-bundle` | Required, decision-bearing | Full harness, candidate native interface, shipped guidance |
| `bare-driver` | Required, decision-bearing | Full harness, same candidate native interface, vendor guidance removed |
| `harness-native-reference` | Required, decision-bearing | Full harness without the evaluated bundle |
| `normalized-facade` | Required, decision-bearing | Full harness, candidate behind the common facade, fixed neutral instructions |
| `driver-only-diagnostic` | Optional, diagnostic | Restricted harness, candidate native interface, vendor guidance removed by default |

The harness-native reference retains normal harness tools. Its declared and
resolved tool inventories must appear in results because those tools may act as
an ad hoc computer interface.

The normalized facade gives every candidate the same frozen neutral
instructions. A release records their digest and exact bytes.

## Pairing and execution

Every comparison pairs trials with the same:

- model;
- provider;
- harness build;
- operating-system image;
- fixture variant; and
- `attempt_index`.

The runtime resets and verifies the complete environment between every trial,
including trials from different profiles in one pair. A reset failure is a
trial status, never permission to reuse state.

Counterbalance profile order across attempts. Each profile must occur equally
often, or differ by at most one occurrence, in each available ordinal position.
Record the planned and actual order so a report can detect drift.

The four required profiles form the decision-bearing candidate comparison.
Missing one makes that candidate's release result incomplete. The optional
driver-only diagnostic answers a restricted Computer-Use 1.0 question and must
remain separate from headline results.

The controlled tool-surface study is a required separate release experiment,
not a profile. Its consolidated, factored, and fine-grained arms keep the
backend, full harness, capability coverage, and guidance condition fixed.

See [About profiles, systems, and comparisons](../docs/explanation/profiles-systems-and-comparisons.md)
for the design rationale and the
[comparison view reference](../docs/reference/comparison-views.md) for report
bindings.
