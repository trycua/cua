#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Lets Cua Spaces create machines in this Google Cloud project without any
# stored key: Cua's issuer signs a 15-minute token for your Cua account, and
# this project's workload identity pool trusts only that account's tokens.
# Everything Cua creates is labelled cua-managed=true. It creates no service
# account and no key.
#
# What it creates (run it again at any time: it changes nothing that is
# already right):
#   - workload identity pool "cua" with the OIDC provider "cua": issuer
#     https://<issuer-host>/a/<account>, the provider's own name as the only
#     allowed audience, and the condition assertion.sub=='account:<account>';
#   - the custom role "cuaSpaces": exactly the Compute Engine permissions
#     below (a test keeps them identical to what Cua checks);
#   - one binding of that role to the identities of your Cua account
#     (principalSet .../attribute.account/account:<account>).
#
# Compute Engine labels can not be used in IAM conditions, so the binding
# has none: the role holds only these permissions, and Cua stops, starts and
# deletes only what carries its labels (`cua cloud audit` lists what it did).
#
# Remove the trust at any time:
#   gcloud iam workload-identity-pools delete cua --location=global --project=<project>
# Disconnecting in Cua stops new tokens at once.
#
# Usage (Google Cloud Shell, or anywhere gcloud is signed in with rights to
# manage IAM in the project):
#   bash cua-trust.sh --project <project-id> --account <cua-account> [--issuer-host oidc.cua.ai]
set -euo pipefail

PROJECT=""
ACCOUNT=""
ISSUER_HOST="oidc.cua.ai"
POOL="cua"
PROVIDER="cua"
ROLE="cuaSpaces"

PERMISSIONS=(
  compute.instances.create
  compute.instances.delete
  compute.instances.get
  compute.instances.list
  compute.instances.start
  compute.instances.stop
  compute.instances.setMetadata
  compute.instances.setLabels
  compute.instances.getSerialPortOutput
  compute.disks.create
  compute.networks.create
  compute.networks.get
  compute.networks.delete
  compute.subnetworks.use
  compute.subnetworks.useExternalIp
)

usage() {
  echo "usage: bash cua-trust.sh --project <project-id> --account <cua-account> [--issuer-host <host>]" >&2
  exit 2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --project) PROJECT="${2:-}"; shift 2 ;;
    --account) ACCOUNT="${2:-}"; shift 2 ;;
    --issuer-host) ISSUER_HOST="${2:-}"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown option: $1" >&2; usage ;;
  esac
done

if [ -z "$PROJECT" ]; then
  PROJECT="$(gcloud config get-value project 2>/dev/null || true)"
fi
[ -n "$PROJECT" ] || { echo "no project: pass --project <project-id>" >&2; exit 2; }
[[ "$ACCOUNT" =~ ^[A-Za-z0-9_-]{6,64}$ ]] || { echo "--account is your Cua account's issuer id (Cua's link has it)" >&2; exit 2; }
[[ "$ISSUER_HOST" =~ ^[a-z0-9.-]+(:[0-9]+)?$ ]] || { echo "--issuer-host is a host name" >&2; exit 2; }

ISSUER="https://${ISSUER_HOST}/a/${ACCOUNT}"
NUMBER="$(gcloud projects describe "$PROJECT" --format='value(projectNumber)')"
PROVIDER_NAME="projects/${NUMBER}/locations/global/workloadIdentityPools/${POOL}/providers/${PROVIDER}"
AUDIENCE="//iam.googleapis.com/${PROVIDER_NAME}"
MAPPING="google.subject=assertion.sub,attribute.account=assertion.sub,attribute.device=assertion.cua_device,attribute.purpose=assertion.cua_purpose"
CONDITION="assertion.sub=='account:${ACCOUNT}'"
MEMBER="principalSet://iam.googleapis.com/projects/${NUMBER}/locations/global/workloadIdentityPools/${POOL}/attribute.account/account:${ACCOUNT}"
PERMS="$(IFS=,; echo "${PERMISSIONS[*]}")"

echo "Project ${PROJECT} (number ${NUMBER}), Cua account ${ACCOUNT}"

gcloud services enable iam.googleapis.com iamcredentials.googleapis.com sts.googleapis.com \
  cloudresourcemanager.googleapis.com compute.googleapis.com --project="$PROJECT" --quiet

if gcloud iam workload-identity-pools describe "$POOL" --location=global --project="$PROJECT" >/dev/null 2>&1; then
  echo "pool ${POOL}: exists"
else
  gcloud iam workload-identity-pools create "$POOL" --location=global --project="$PROJECT" \
    --display-name="Cua Spaces" --description="Tokens Cua's issuer signs for Cua account ${ACCOUNT}" --quiet
fi

if gcloud iam workload-identity-pools providers describe "$PROVIDER" --workload-identity-pool="$POOL" \
  --location=global --project="$PROJECT" >/dev/null 2>&1; then
  gcloud iam workload-identity-pools providers update-oidc "$PROVIDER" --workload-identity-pool="$POOL" \
    --location=global --project="$PROJECT" --issuer-uri="$ISSUER" --allowed-audiences="$AUDIENCE" \
    --attribute-mapping="$MAPPING" --attribute-condition="$CONDITION" --quiet
else
  gcloud iam workload-identity-pools providers create-oidc "$PROVIDER" --workload-identity-pool="$POOL" \
    --location=global --project="$PROJECT" --display-name="Cua" --issuer-uri="$ISSUER" \
    --allowed-audiences="$AUDIENCE" --attribute-mapping="$MAPPING" --attribute-condition="$CONDITION" --quiet
fi

if gcloud iam roles describe "$ROLE" --project="$PROJECT" >/dev/null 2>&1; then
  gcloud iam roles update "$ROLE" --project="$PROJECT" --permissions="$PERMS" --stage=GA --quiet >/dev/null
else
  gcloud iam roles create "$ROLE" --project="$PROJECT" --title="Cua Spaces" \
    --description="Create and manage the machines Cua Spaces labels cua-managed=true" \
    --permissions="$PERMS" --stage=GA --quiet >/dev/null
fi

gcloud projects add-iam-policy-binding "$PROJECT" --member="$MEMBER" \
  --role="projects/${PROJECT}/roles/${ROLE}" --condition=None --quiet >/dev/null

echo
echo "Done. Cua's tokens for account ${ACCOUNT} can now use project ${PROJECT}."
echo "Project number: ${NUMBER}"
echo "Provider:       ${AUDIENCE}"
echo "Next: give Cua the project number (cua cloud connect gcp ... --project-number ${NUMBER} --check)."
