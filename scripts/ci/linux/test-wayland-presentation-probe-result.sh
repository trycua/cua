#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
probe="$(mktemp)"
trap 'rm -f "${probe}"' EXIT

printf '%s\n' '{"kind":"probe","presentation_supported":true,"presentation_feedback_observed":true,"presentation_clock_comparable":true}' > "${probe}"
[[ "$(bash "${script_dir}/wayland-presentation-probe-result.sh" 0 "${probe}")" == ready ]]

printf '%s\n' '{"kind":"probe","presentation_supported":true,"presentation_feedback_observed":true,"presentation_clock_comparable":false}' > "${probe}"
[[ "$(bash "${script_dir}/wayland-presentation-probe-result.sh" 0 "${probe}")" == clock_mismatch ]]

printf '%s\n' '{"kind":"probe","presentation_supported":true,"presentation_feedback_observed":false}' > "${probe}"
[[ "$(bash "${script_dir}/wayland-presentation-probe-result.sh" 3 "${probe}")" == feedback_unavailable ]]

: > "${probe}"
[[ "$(bash "${script_dir}/wayland-presentation-probe-result.sh" 3 "${probe}")" == protocol_unavailable ]]
[[ "$(bash "${script_dir}/wayland-presentation-probe-result.sh" 1 "${probe}")" == error ]]
[[ "$(bash "${script_dir}/wayland-presentation-probe-result.sh" 0 "${probe}")" == error ]]
