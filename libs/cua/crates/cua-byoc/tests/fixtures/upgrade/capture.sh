#!/bin/bash
# capture.sh <cua binary> <out dir> <aws endpoint> <cmd...>: runs a released
# cua in the throwaway HOME <out dir> against fake clouds, never the real
# home, keychain or AWS profiles.
CUA=$1; OUT=$2; AWS=$3; shift 3
mkdir -p "$OUT"
cat > "$OUT/aws-config" <<C
[profile upgrade-fake]
region = us-west-2
C
cat > "$OUT/aws-credentials" <<C
[upgrade-fake]
aws_access_key_id = AKIDUPGRADEFAKE
aws_secret_access_key = fake
C
exec env -i PATH=/usr/bin:/bin HOME="$OUT" CUA_HOME="$OUT/.cua" \
  CUA_CREDENTIAL_STORE=file DO_NOT_TRACK=1 CUA_TELEMETRY=0 \
  AWS_ENDPOINT_URL="$AWS" AWS_CONFIG_FILE="$OUT/aws-config" \
  AWS_SHARED_CREDENTIALS_FILE="$OUT/aws-credentials" \
  "$CUA" "$@"
