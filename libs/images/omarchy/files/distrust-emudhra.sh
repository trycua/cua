#!/bin/sh
# CUA-1948: distrust the roots the compromised eMudhra intermediates chain to.
# Arch's ca-certificates-mozilla >= 3.131.1-1 already drops them; on an older
# archive snapshot remove them from the p11-kit trust store (a no-op once gone).
set -eu
bl=/etc/ca-certificates/trust-source/blocklist
mkdir -p "$bl"
for label in "emSign ECC Root CA - G3" "AAA Certificate Services"; do
  trust list --filter=ca-anchors | awk -v want="$label" '
    /^pkcs11:/ { uri=$0 }
    /^[ \t]+label: / { sub(/^[ \t]+label: /, ""); if ($0 == want) print uri }' |
  while read -r uri; do
      f="$bl/$(echo "$label" | tr -c "A-Za-z0-9" _).pem"
      [ -e "$f" ] || trust extract --filter="$uri" --format=pem-bundle "$f"
    done
done
update-ca-trust
