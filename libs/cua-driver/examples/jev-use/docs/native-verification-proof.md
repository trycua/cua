# Native jev-use intermediate verification proof

Sanitized text proof for the native recipe change that ends whole-task waits
after an independently witnessed counter increment or Large radio selection.
This is not a new Jev or provider benchmark. No issue-closing claim is made.
Refs [#4336](https://github.com/trycua/cua/issues/4336) as complementary
timing work.

## Identities

| Item | Value |
| --- | --- |
| Immutable poller baseline | `62cdfbcd9356cf2e1b47ab09fee94eddd1d881cc` |
| Fresh package base | `0f29c142d7fe3e05ea0ce276cee11b3a9725ba01` |
| Candidate source | original live-tested six-file candidate; production bytes unchanged on the fresh package base |
| Released Cua Driver | 0.30.4 |
| Offline runtime | Python 3.12.9, macOS, deterministic mock chooser |

`poll_oracle` at the fresh base is byte-identical to the immutable baseline.
Production candidate files were not rewritten for this proof. No new Jev calls
were made; the mock path constructed no paid provider client.

## Offline policy corpus

`verify_native_waits.py` compiles baseline `poll_oracle` from Git and runs the
production candidate poller. Nine delayed-effect cells (18 terminals) all
verify. Virtual policy latency is 25.5s baseline versus 9.3s candidate. Those
numbers are fake-clock sleeps, not live or model time.

## Live Python / macOS A/B

Four interleaved baseline/candidate pairs per task, task order rotated, on
fresh fixture snapshots. 24 trials completed; 56 dispatched mutations have
successful post-dispatch receipts; every trial has a final window capture.
Focus monitors recorded 4,605 samples with zero fixture-foreground samples and
zero activation notifications. Sampling does not prove the absence of every
transient focus event.

| Task | Baseline median task | Candidate median task |
| --- | ---: | ---: |
| counter | 10.770s | 6.444s |
| choose-size | 7.564s | 5.485s |
| save-note | 8.148s | 8.137s |

Four trials per cell do not establish a general performance guarantee.

Counter pair 3 is retained: candidate task time was 13.083s versus baseline
10.889s despite removing 4.050s of verification waiting. Measured driver-call
latency increased on that pair. The OS, daemon, or delivery cause was not
instrumented and is unproved.

## Fixture and monitor limits

The live harness used a patched accessory AppKit fixture launched with
`open -g -n -W`, a 0.5s startup allowance, an owned focus monitor, and
in-process scoped driver calls. That is not a claim that the stock repository
fixture is canonical focus proof.

## Gaps

Python/macOS live only. TypeScript has unit and integration coverage, not a
new native live pass. Other operating systems, generic real-app
postconditions, operator concurrency, and delayed-effect live fixtures remain
unproved. The fixture oracle does not demonstrate application durability or
every downstream side effect.
