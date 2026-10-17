#!/usr/bin/env bash
# Classify the fixture probe without treating an incomparable clock as latency.
set -euo pipefail

status="$1"
journal="$2"
probe=""
if [[ -f "${journal}" ]]; then
  probe="$(jq -c 'select(.kind == "probe")' "${journal}" 2>/dev/null | tail -1)" || probe=""
fi

case "${status}" in
  0)
    if [[ -z "${probe}" ]] || [[ "$(jq -r '.presentation_feedback_observed' <<< "${probe}")" != true ]]; then
      echo error
    elif [[ "$(jq -r '.presentation_clock_comparable' <<< "${probe}")" == true ]]; then
      echo ready
    else
      echo clock_mismatch
    fi
    ;;
  3)
    if [[ -n "${probe}" ]] && [[ "$(jq -r '.presentation_supported' <<< "${probe}")" == true ]]; then
      echo feedback_unavailable
    else
      echo protocol_unavailable
    fi
    ;;
  *) echo error ;;
esac
