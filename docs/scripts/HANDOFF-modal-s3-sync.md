# Handoff: Deploy & Test Modal S3 Sync

## Context

PR #1430 (`feat/modal-s3-sync` branch in `trycua/cua`) adds a `sync_to_s3()` function to `docs/scripts/modal_app.py`. This function uploads generated documentation and code search databases from Modal volumes to S3, bridging the gap between Modal (which generates the DBs on a schedule) and a K8s-hosted MCP server (which serves them).

### Architecture

```
Modal (generates DBs daily) → sync_to_s3() → S3 → K8s CronJob (pulls from S3) → PVC → MCP Server
```

### What was built

1. **`sync_to_s3()` function** in `docs/scripts/modal_app.py`:
   - Uploads SQLite + LanceDB files from two Modal volumes to `s3://trycua-docs-mcp-data/`
   - Uses **Modal OIDC federation** (no static AWS keys) — Modal auto-injects `MODAL_IDENTITY_TOKEN`, which is exchanged via STS `AssumeRoleWithWebIdentity` for temporary credentials
   - IAM role: `arn:aws:iam::296062593712:role/modal-docs-mcp-write-role` (already provisioned via Terraform in `trycua/cloud` repo)

2. **Integration into scheduled pipelines**:
   - `scheduled_crawl()` (6 AM UTC) — calls `sync_to_s3()` after regenerating docs databases
   - `scheduled_code_index()` (5 AM UTC) — calls `sync_to_s3()` after aggregating code databases

3. **S3 bucket** (`trycua-docs-mcp-data`) already exists with versioning, encryption, and cross-region replication. Currently has `docs_db/` data from a prior manual upload.

### What needs testing

The code has NOT been deployed yet. The previous environment could not reach PyPI (SSL handshake failures), so `modal` CLI was never installed.

---

## Your Task

### 1. Install Modal CLI

```bash
pip install modal
```

### 2. Authenticate Modal

The Modal token should be available in `~/.modal/config.toml`. If not, ask the user to provide it or run:

```bash
modal token set --token-id <ask-user> --token-secret <ask-user>
```

### 3. Checkout the branch

```bash
cd /path/to/cua  # the trycua/cua repo
git fetch origin
git checkout feat/modal-s3-sync
```

### 4. Deploy the Modal app

```bash
modal deploy docs/scripts/modal_app.py
```

This should succeed. The app name is `cua-docs-mcp` — it will register all functions including the new `sync_to_s3`.

### 5. Test `sync_to_s3` manually

```bash
modal run docs/scripts/modal_app.py::sync_to_s3
```

**Expected output:**
- "Syncing databases to s3://trycua-docs-mcp-data/ ..."
- Upload lines for docs SQLite, docs LanceDB directory, code SQLite, and code LanceDB directory
- "S3 sync complete: N files uploaded to s3://trycua-docs-mcp-data/"

**If OIDC auth fails** with an STS error about the role trust policy, check:
- The Modal app name must be `cua-docs-mcp` (defined at top of `modal_app.py`)
- The role trust policy restricts to `app_name:cua-docs-mcp` — if the app name changed, the OIDC claim won't match
- Verify the role exists: `aws iam get-role --role-name modal-docs-mcp-write-role`

**If volumes are empty** (no databases to upload), you'll need to generate them first:
```bash
# Generate docs databases (takes ~30 min)
modal run docs/scripts/modal_app.py --skip-code

# Generate code databases (takes ~1-2 hours)  
modal run docs/scripts/modal_app.py --skip-docs
```

### 6. Verify S3 contents

```bash
aws s3 ls s3://trycua-docs-mcp-data/ --recursive --region us-west-2
```

**Expected structure:**
```
docs_db/docs.sqlite
docs_db/docs.lance/...
code_db/code_index.sqlite
code_db/code_index.lancedb/...
```

### 7. Report results

Let the user know:
- Whether `modal deploy` succeeded
- Whether `sync_to_s3` ran and how many files were uploaded
- The S3 listing output
- Any errors encountered

---

## Key Files

| File | Purpose |
|------|---------|
| `docs/scripts/modal_app.py` | Modal app — crawling, indexing, S3 sync, MCP server |
| (cloud repo) `terraform/aws/docs-mcp-storage/main.tf` | S3 bucket, OIDC role, readonly IAM user |
| (cloud repo) `terraform/aws/oidc/modal-oidc.tf` | Modal OIDC provider |

## AWS Details

| Resource | Value |
|----------|-------|
| S3 Bucket | `trycua-docs-mcp-data` |
| Region | `us-west-2` |
| Write Role ARN | `arn:aws:iam::296062593712:role/modal-docs-mcp-write-role` |
| Readonly Creds Secret | `docs-mcp/readonly-credentials` (in AWS Secrets Manager) |
| Modal App Name | `cua-docs-mcp` |
| Modal Workspace ID | `ac-3LfmQEOVnLXl4ns0YBxNuA` |
