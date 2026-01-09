## Build

Replace `enhanced-kiln-453008-s4` (PROJECT_ID) and `us-central1` (REGION) as needed.

```bash
# Enable Artifact Registry and create a repo once (if not already):
gcloud services enable artifactregistry.googleapis.com
# Example: create a Docker repo
# gcloud artifacts repositories create bench --repository-format=docker --location=us-central1 --description="Bench images"

# Multi-arch build (amd64 + arm64) and push to Artifact Registry
PROJECT_ID="enhanced-kiln-453008-s4"
REGION="us-central1"
IMAGE="${REGION}-docker.pkg.dev/${PROJECT_ID}/bench/py311-playwright-bench:latest"

# Authenticate Docker to Artifact Registry for your region
gcloud auth configure-docker "${REGION}-docker.pkg.dev"

# Build and push a multi-arch image manifest for linux/amd64 and linux/arm64
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  -t "$IMAGE" \
  --push \
  .
```

## Run

Set `GCP_IMAGE_URI` to the image you built.

```bash
export GCP_IMAGE_URI="us-central1-docker.pkg.dev/enhanced-kiln-453008-s4/bench/py311-playwright-bench:latest"
```

Run the batch job:

```bash
cb batch solve tasks/click_env 16 --parallelism 8
```
