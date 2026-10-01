"""Batch enrichment pipeline: search → Bedrock Batch → S3.

Three independent phases:
  1. search  — do N DuckDuckGo searches per app, write search_results.jsonl
  2. submit  — format as Bedrock batch input, upload to S3, create inference job
  3. fetch   — download S3 output, parse JSON, write enriched_software.jsonl
"""

from __future__ import annotations

import asyncio
import gzip
import json
import logging
import time
import uuid
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Prompts
# ---------------------------------------------------------------------------

ENRICH_SYSTEM_PROMPT = """\
You are a software metadata enrichment agent. Given an app name, website, and
web search results, output a single JSON object (no markdown, no explanation)
with these exact fields:

{
  "id": "<slug: lowercase, hyphens>",
  "name": "<canonical name>",
  "description": "<1-2 sentence description>",
  "website": "<url>",
  "icon_url": "<url or null>",
  "categories": ["<category>", ...],
  "tags": ["<tag>", ...],
  "os_support": ["linux"|"windows"|"macos"|"android", ...],
  "app_type": "standalone"|"cli"|"library"|"webapp"|"both",
  "requires_payment": <true if CUA agent needs credit card to use core UI, else false>,
  "foss": <true if OSI-approved open source with public repo, else false>,
  "gh_repo": "<github/gitlab/codeberg url or null>",
  "self_hostable": <true if runs fully locally with no cloud dependency>,
  "requires_hardware": <true if requires special hardware>,
  "package_managers": {
    "apt": "<id or null>",
    "snap": "<id or null>",
    "flatpak": "<id or null>",
    "brew": "<id or null>",
    "choco": "<id or null>",
    "winget": "<id or null>"
  },
  "download_url": "<direct download url or null>",
  "hallucinated": <true if you cannot find credible evidence this software exists, else false>,
  "hallucination_reason": "<brief reason if hallucinated=true, else null>"
}

app_type rules:
- "standalone": installable desktop GUI or mobile app (has a window/UI)
- "cli": command-line or TUI software (no GUI window, runs in terminal)
- "library": software dependency/SDK for development (pip, npm, cargo packages etc.)
- "webapp": only accessible via browser, no installable client
- "both": has both an installable client AND a web interface

hallucinated rules:
- Set true if: website 404s or doesn't exist, no search results mention it, name looks
  like a generic description rather than a real product, or you find no credible source
  (vendor page, GitHub, review, news article) confirming it exists.
- Set false if you find ANY credible evidence (official site, repo, app store listing, etc.)

Be precise. Never guess package manager IDs — use null if unknown.
Output ONLY the JSON object.
"""


def _already_enriched(enriched_path: Path | None) -> set[str]:
    """Return lowercase names already present in enriched_software.jsonl."""
    done: set[str] = set()
    if not enriched_path or not enriched_path.exists():
        return done
    with open(enriched_path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                done.add(json.loads(line).get("name", "").lower().strip())
            except json.JSONDecodeError:
                pass
    return done


def _search_queries(name: str, website: str, n: int) -> list[str]:
    q = f'"{name}"'
    base = [
        f"{q} software",
        f"{q} pricing open source github",
        f"{q} install linux windows macos apt brew choco winget",
        f"{q} {website} features review",
        f"site:github.com {q}",
    ]
    return base[:n]


# ---------------------------------------------------------------------------
# Phase 1: Search
# ---------------------------------------------------------------------------


async def _searxng_search(
    query: str,
    searxng_url: str,
    session: Any,
    max_results: int = 5,
) -> list[dict]:
    """Single SearXNG JSON search — hits all configured engines in parallel."""
    try:
        params = {"q": query, "format": "json"}
        async with session.get(f"{searxng_url.rstrip('/')}/search", params=params) as resp:
            if resp.status != 200:
                return []
            data = await resp.json(content_type=None)
            results = data.get("results", [])[:max_results]
            return [
                {
                    "title": r.get("title", ""),
                    "body": r.get("content", ""),
                    "href": r.get("url", ""),
                }
                for r in results
            ]
    except Exception as e:
        logger.debug("SearXNG search failed for %r: %s", query, e)
        return []


class _TokenBucket:
    """Simple async token bucket — refills at `rate` tokens/sec."""

    def __init__(self, rate: float):
        self._rate = rate
        self._tokens = rate
        self._last = time.monotonic()
        self._lock = asyncio.Lock()

    async def acquire(self) -> None:
        while True:
            async with self._lock:
                now = time.monotonic()
                self._tokens = min(self._rate, self._tokens + (now - self._last) * self._rate)
                self._last = now
                if self._tokens >= 1:
                    self._tokens -= 1
                    return
            await asyncio.sleep(1.0 / self._rate)


async def _brave_search(
    query: str,
    api_key: str,
    session: Any,
    max_results: int = 5,
    bucket: "_TokenBucket | None" = None,
) -> list[dict]:
    """Brave Search API — high quality, rate-limited via token bucket."""
    if bucket:
        await bucket.acquire()
    try:
        headers = {
            "Accept": "application/json",
            "Accept-Encoding": "gzip",
            "X-Subscription-Token": api_key,
        }
        params = {"q": query, "count": max_results}
        async with session.get(
            "https://api.search.brave.com/res/v1/web/search", headers=headers, params=params
        ) as resp:
            if resp.status == 429:
                logger.warning("Brave API rate limit hit for %r", query)
                return []
            if resp.status != 200:
                logger.debug("Brave API %d for %r", resp.status, query)
                return []
            data = await resp.json(content_type=None)
            results = data.get("web", {}).get("results", [])[:max_results]
            return [
                {
                    "title": r.get("title", ""),
                    "body": r.get("description", ""),
                    "href": r.get("url", ""),
                }
                for r in results
            ]
    except Exception as e:
        logger.debug("Brave API search failed for %r: %s", query, e)
        return []


async def _search_one(
    entry: dict,
    n: int,
    semaphore: asyncio.Semaphore,
    *,
    searxng_url: str | None = None,
    brave_api_key: str | None = None,
    brave_bucket: "_TokenBucket | None" = None,
    delay: float = 0.05,
    session: Any = None,
) -> dict:
    name = entry.get("name", "")
    website = entry.get("website", "")
    queries = _search_queries(name, website, n)
    search_results: dict[str, list] = {}

    async with semaphore:
        if brave_api_key and session is not None:
            # Sequential queries per app — keeps total req/s = concurrency (not concurrency*n)
            results_list = []
            for q in queries:
                results_list.append(
                    await _brave_search(q, brave_api_key, session, bucket=brave_bucket)
                )
            for q, results in zip(queries, results_list):
                search_results[q] = results
        elif searxng_url and session is not None:
            for q in queries:
                search_results[q] = await _searxng_search(q, searxng_url, session)
                if delay:
                    await asyncio.sleep(delay)
        else:
            try:
                from duckduckgo_search import AsyncDDGS

                async with AsyncDDGS() as ddgs:
                    for q in queries:
                        try:
                            results = await ddgs.atext(q, max_results=5)
                            search_results[q] = results or []
                        except Exception as e:
                            logger.debug("Search failed for %r: %s", q, e)
                            search_results[q] = []
                        await asyncio.sleep(0.5)
            except Exception as e:
                logger.warning("Search error for %s: %s", name, e)

    return {**entry, "_search_results": search_results}


async def run_search(
    input_path: Path,
    output_path: Path,
    *,
    n: int = 3,
    concurrency: int = 10,
    enriched_path: Path | None = None,
    searxng_url: str | None = None,
    brave_api_key: str | None = None,
) -> None:
    """Phase 1: gather N web searches per app entry."""
    from .onet import read_jsonl

    raw_entries = read_jsonl(input_path)

    # Skip already searched (only if they actually have results — empty = needs retry)
    done: set[str] = set()
    if output_path.exists():
        with open(output_path, encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    e = json.loads(line)
                    total_results = sum(len(v) for v in e.get("_search_results", {}).values())
                    if total_results > 0:
                        done.add(e.get("name", "").lower().strip())
                except json.JSONDecodeError:
                    pass

    # Also skip already fully enriched (from prior agent-based run or prior batch)
    done |= _already_enriched(enriched_path)

    remaining = [e for e in raw_entries if e.get("name", "").lower().strip() not in done]
    logger.info(
        "Searching %d entries (%d already done/enriched, n=%d, concurrency=%d)",
        len(remaining),
        len(done),
        n,
        concurrency,
    )

    sem = asyncio.Semaphore(concurrency)
    output_path.parent.mkdir(parents=True, exist_ok=True)

    if brave_api_key:
        provider = "brave api"
    elif searxng_url:
        provider = f"searxng({searxng_url})"
    else:
        provider = "duckduckgo"
    logger.info("Search provider: %s", provider)

    if brave_api_key or searxng_url:
        import aiohttp

        connector = aiohttp.TCPConnector(
            limit=concurrency * 3
        )  # N queries fired in parallel per app
        headers = {"X-Forwarded-For": "127.0.0.1"}
        brave_bucket = _TokenBucket(45) if brave_api_key else None  # 45 req/s < 50/s limit
        async with aiohttp.ClientSession(connector=connector, headers=headers) as session:

            async def _run_one(entry: dict) -> None:
                result = await _search_one(
                    entry,
                    n,
                    sem,
                    searxng_url=searxng_url,
                    brave_api_key=brave_api_key,
                    brave_bucket=brave_bucket,
                    session=session,
                )
                with open(output_path, "a", encoding="utf-8") as f:
                    f.write(json.dumps(result, default=str) + "\n")
                logger.info("Searched: %s", entry.get("name"))

            await asyncio.gather(*[_run_one(e) for e in remaining])
    else:

        async def _run_one(entry: dict) -> None:  # type: ignore[no-redef]
            result = await _search_one(entry, n, sem)
            with open(output_path, "a", encoding="utf-8") as f:
                f.write(json.dumps(result, default=str) + "\n")
            logger.info("Searched: %s", entry.get("name"))

        await asyncio.gather(*[_run_one(e) for e in remaining])

    logger.info("Search complete. Results in %s", output_path)


# ---------------------------------------------------------------------------
# Phase 2: Bedrock Batch Submit
# ---------------------------------------------------------------------------


def _make_batch_record(entry: dict, model_id: str) -> dict:
    name = entry.get("name", "unknown")
    website = entry.get("website", "")
    searches = entry.get("_search_results", {})
    category = entry.get("category", "")
    os_hint = entry.get("os_support", [])

    search_text = ""
    for query, results in searches.items():
        search_text += f"\nSearch: {query}\n"
        for r in results[:3]:
            title = r.get("title", "")
            body = r.get("body", "")[:300]
            search_text += f"  - {title}: {body}\n"

    user_prompt = (
        f"App: {name}\nWebsite: {website}\nCategory hint: {category}\n"
        f"Known OS: {os_hint}\n\nWeb search results:{search_text}\n\n"
        f"Output the enriched JSON object."
    )

    return {
        "recordId": f"{name.lower().replace(' ', '-')[:80]}-{uuid.uuid4().hex[:8]}",
        "modelInput": {
            "anthropic_version": "bedrock-2023-05-31",
            "max_tokens": 1024,
            "system": ENRICH_SYSTEM_PROMPT,
            "messages": [{"role": "user", "content": user_prompt}],
        },
    }


def submit_batch(
    input_path: Path,
    *,
    s3_bucket: str,
    s3_prefix: str = "cua-sandbox-apps/enrich",
    model_id: str = "us.anthropic.claude-haiku-4-5-20251001-v1:0",
    region: str = "us-east-1",
    role_arn: str | None = None,
    enriched_path: Path | None = None,
) -> str:
    """Phase 2: upload batch input to S3, create Bedrock batch inference job.

    Returns the job ARN.
    """
    import boto3

    with open(input_path, encoding="utf-8") as f:
        entries = [json.loads(line) for line in f if line.strip()]

    # Skip already enriched entries
    already_done = _already_enriched(enriched_path)
    if already_done:
        before = len(entries)
        entries = [e for e in entries if e.get("name", "").lower().strip() not in already_done]
        logger.info(
            "Skipping %d already-enriched entries (%d remaining)",
            before - len(entries),
            len(entries),
        )

    logger.info("Preparing %d batch records for %s", len(entries), model_id)
    records = [_make_batch_record(e, model_id) for e in entries]

    # Write to temp JSONL
    job_id = uuid.uuid4().hex[:12]
    input_key = f"{s3_prefix}/input/{job_id}/records.jsonl"
    output_prefix = f"s3://{s3_bucket}/{s3_prefix}/output/{job_id}/"

    s3 = boto3.client("s3", region_name=region)
    body = "\n".join(json.dumps(r) for r in records).encode("utf-8")
    s3.put_object(Bucket=s3_bucket, Key=input_key, Body=body)
    logger.info("Uploaded %d records to s3://%s/%s", len(records), s3_bucket, input_key)

    bedrock = boto3.client("bedrock", region_name=region)

    create_kwargs: dict[str, Any] = {
        "jobName": f"cua-enrich-{job_id}",
        "modelId": model_id,
        "inputDataConfig": {
            "s3InputDataConfig": {
                "s3Uri": f"s3://{s3_bucket}/{input_key}",
                "s3InputFormat": "JSONL",
            }
        },
        "outputDataConfig": {
            "s3OutputDataConfig": {
                "s3Uri": output_prefix,
            }
        },
    }
    if role_arn:
        create_kwargs["roleArn"] = role_arn

    response = bedrock.create_model_invocation_job(**create_kwargs)
    job_arn = response["jobArn"]
    logger.info("Batch job created: %s", job_arn)
    logger.info("Output will be at: %s", output_prefix)
    return job_arn


def get_batch_status(job_arn: str, region: str = "us-east-1") -> dict:
    import boto3

    bedrock = boto3.client("bedrock", region_name=region)
    resp = bedrock.get_model_invocation_job(jobIdentifier=job_arn)
    return {
        "status": resp.get("status"),
        "jobArn": resp.get("jobArn"),
        "jobName": resp.get("jobName"),
        "submitTime": str(resp.get("submitTime", "")),
        "endTime": str(resp.get("endTime", "")),
        "outputDataConfig": resp.get("outputDataConfig", {}),
    }


# ---------------------------------------------------------------------------
# Phase 3: Fetch from S3
# ---------------------------------------------------------------------------


def fetch_results(
    job_arn: str,
    output_path: Path,
    *,
    region: str = "us-east-1",
    poll: bool = True,
    poll_interval: int = 60,
) -> int:
    """Phase 3: wait for job completion, download S3 results, write enriched JSONL.

    Returns count of successfully parsed entries.
    """
    import boto3

    bedrock = boto3.client("bedrock", region_name=region)
    s3 = boto3.client("s3", region_name=region)

    # Poll until done
    if poll:
        while True:
            resp = bedrock.get_model_invocation_job(jobIdentifier=job_arn)
            status = resp.get("status")
            logger.info("Job status: %s", status)
            if status in ("Completed", "Failed", "Stopped", "Expired"):
                break
            time.sleep(poll_interval)
    else:
        resp = bedrock.get_model_invocation_job(jobIdentifier=job_arn)

    status = resp.get("status")
    if status != "Completed":
        logger.error("Job ended with status: %s", status)
        return 0

    # Get S3 output location
    s3_uri = resp["outputDataConfig"]["s3OutputDataConfig"]["s3Uri"]
    # s3://bucket/prefix/
    s3_uri = s3_uri.rstrip("/")
    bucket = s3_uri.split("/")[2]
    prefix = "/".join(s3_uri.split("/")[3:])

    logger.info("Downloading results from %s", s3_uri)

    paginator = s3.get_paginator("list_objects_v2")
    output_path.parent.mkdir(parents=True, exist_ok=True)
    count = 0
    errors = 0

    # Load already-enriched names so we don't write duplicates
    already_done = _already_enriched(output_path)
    logger.info("Skipping %d already-enriched entries on write", len(already_done))

    with open(output_path, "a", encoding="utf-8") as out_f:
        for page in paginator.paginate(Bucket=bucket, Prefix=prefix):
            for obj in page.get("Contents", []):
                key = obj["Key"]
                if not key.endswith(".jsonl") and not key.endswith(".jsonl.gz"):
                    continue

                logger.debug("Fetching %s", key)
                s3_obj = s3.get_object(Bucket=bucket, Key=key)
                raw = s3_obj["Body"].read()

                if key.endswith(".gz"):
                    raw = gzip.decompress(raw)

                for line in raw.decode("utf-8").splitlines():
                    line = line.strip()
                    if not line:
                        continue
                    try:
                        record = json.loads(line)
                        # Bedrock batch output format
                        model_output = record.get("modelOutput", {})
                        content = model_output.get("content", [])
                        text = ""
                        for block in content:
                            if block.get("type") == "text":
                                text = block.get("text", "")
                                break

                        # Parse the JSON the model returned
                        text = text.strip()
                        if text.startswith("```"):
                            text = text.split("```")[1]
                            if text.startswith("json"):
                                text = text[4:]
                        entry = json.loads(text)
                        name_key = entry.get("name", "").lower().strip()
                        if name_key in already_done:
                            continue
                        already_done.add(name_key)
                        out_f.write(json.dumps(entry, default=str) + "\n")
                        count += 1
                    except (json.JSONDecodeError, KeyError) as e:
                        logger.debug("Parse error on record: %s", e)
                        errors += 1

    logger.info("Fetched %d entries (%d errors) → %s", count, errors, output_path)
    return count
