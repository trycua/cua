#!/usr/bin/env bash
# Compare a verifier's exit status with the expected outcome.
#
#   expect_verification_result.sh <pass|fail> <exit-status> <description>
#
# "pass" is the release gate: the verifier must succeed. "fail" is a negative
# control: the verifier must reject a known-bad input (for example the
# withdrawn, unsigned 0.28.3 macOS archives), which proves the gate can fail.
set -euo pipefail

expect="${1:?expected outcome (pass or fail) is required}"
status="${2:?verifier exit status is required}"
description="${3:-verification}"

case "$expect" in
    pass)
        if [[ "$status" == "0" ]]; then
            echo "${description}: passed as required."
            exit 0
        fi
        echo "::error title=Release signature gate::${description} failed (exit ${status}); publication and installer baking are blocked."
        exit 1
        ;;
    fail)
        if [[ "$status" == "0" ]]; then
            echo "::error title=Negative control::${description} passed, but this input is known to be unsigned; the verifier cannot detect a broken release."
            exit 1
        fi
        echo "${description}: rejected as expected for this negative control (exit ${status})."
        exit 0
        ;;
    *)
        echo "::error::unknown expected outcome '${expect}'; use pass or fail"
        exit 2
        ;;
esac
