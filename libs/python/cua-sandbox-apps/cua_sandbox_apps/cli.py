"""CLI for cua-sandbox-apps."""

from __future__ import annotations

import asyncio
import logging
import os
from pathlib import Path

import click

# Load .env from package root (Bedrock / API key config)
_env_file = Path(__file__).parent.parent / ".env"
if _env_file.exists():
    for _line in _env_file.read_text().splitlines():
        _line = _line.strip()
        if _line and not _line.startswith("#") and "=" in _line:
            _k, _v = _line.split("=", 1)
            os.environ.setdefault(_k.strip(), _v.strip())

# Default data directory — relative to package
DEFAULT_DATA_DIR = Path(__file__).parent / "data"


def _setup_logging(verbose: bool) -> None:
    level = logging.DEBUG if verbose else logging.INFO
    logging.basicConfig(
        level=level,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        datefmt="%H:%M:%S",
    )


@click.group()
@click.option("-v", "--verbose", is_flag=True, help="Enable debug logging")
def cli(verbose: bool) -> None:
    """CUA Sandbox Apps — universal app catalog and installer pipeline."""
    _setup_logging(verbose)


@cli.group()
def discover() -> None:
    """Phase 1: Discover software across occupations."""
    pass


@discover.command("onet")
@click.option("--output", type=click.Path(), default=None, help="Output JSONL path")
def discover_onet(output: str | None) -> None:
    """Load O*NET occupation groups (static data, no LLM)."""
    from .discovery.onet import generate_soc_groups_as_occupations, write_jsonl

    out = Path(output) if output else DEFAULT_DATA_DIR / "occupations.jsonl"
    occupations = generate_soc_groups_as_occupations()
    # Overwrite (not append) for this static step
    out.parent.mkdir(parents=True, exist_ok=True)
    out.unlink(missing_ok=True)
    write_jsonl(occupations, out)
    click.echo(f"Wrote {len(occupations)} occupation groups to {out}")


@discover.command("software")
@click.option("--input", "input_path", type=click.Path(exists=True), default=None)
@click.option("--output", type=click.Path(), default=None)
@click.option("--target", type=int, default=50_000, help="Target number of entries")
@click.option("--model", type=str, default="haiku", help="Claude model (haiku, sonnet, opus)")
@click.option("--max-parallel", type=int, default=22, help="Max parallel agents")
@click.option("--start-angle", type=int, default=0, help="Search angle index to start from (0-7)")
def discover_software(
    input_path: str | None,
    output: str | None,
    target: int,
    model: str,
    max_parallel: int,
    start_angle: int,
) -> None:
    """Discover software per occupation group using Claude agents with WebSearch.

    All 22 occupation groups run fully in parallel. Each agent gets WebSearch
    and a submit_apps tool to ingest entries. Outer loop re-prompts until target.
    Fully resumable — deduplicates on ingress.
    """
    from .discovery.discover import run_discovery
    from .discovery.onet import read_jsonl

    inp = Path(input_path) if input_path else DEFAULT_DATA_DIR / "occupations.jsonl"
    out = Path(output) if output else DEFAULT_DATA_DIR / "raw_software.jsonl"

    if not inp.exists():
        click.echo(f"Input not found: {inp}. Run 'discover onet' first.", err=True)
        raise SystemExit(1)

    occupations = read_jsonl(inp)
    click.echo(
        f"Discovery: {len(occupations)} groups, target={target}, model={model}, max_parallel={max_parallel}, start_angle={start_angle}"
    )
    asyncio.run(
        run_discovery(
            occupations,
            out,
            target=target,
            model=model,
            max_parallel=max_parallel,
            start_angle=start_angle,
        )
    )
    click.echo(f"Results in {out}")


@discover.command("enrich")
@click.option("--input", "input_path", type=click.Path(exists=True), default=None)
@click.option("--output", type=click.Path(), default=None)
@click.option("--concurrency", type=int, default=5)
@click.option("--model", type=str, default="haiku", help="Claude model (haiku, sonnet, opus)")
def discover_enrich(
    input_path: str | None, output: str | None, concurrency: int, model: str
) -> None:
    """Enrich raw software entries with detailed metadata."""
    from .discovery.enrich import run_enrichment

    inp = Path(input_path) if input_path else DEFAULT_DATA_DIR / "raw_software.jsonl"
    out = Path(output) if output else DEFAULT_DATA_DIR / "enriched_software.jsonl"

    if not inp.exists():
        click.echo(f"Input not found: {inp}. Run 'discover software' first.", err=True)
        raise SystemExit(1)

    click.echo(f"Enriching entries from {inp} (concurrency={concurrency}, model={model})")
    asyncio.run(run_enrichment(inp, out, concurrency=concurrency, model=model))
    click.echo(f"Results appended to {out}")


@discover.command("search")
@click.option("--input", "input_path", type=click.Path(exists=True), default=None)
@click.option("--output", type=click.Path(), default=None)
@click.option("--n", type=int, default=3, help="Number of searches per app")
@click.option("--concurrency", type=int, default=50, help="Concurrent searches")
@click.option(
    "--searxng-url",
    default="http://localhost:8080",
    show_default=True,
    help="SearXNG base URL (set empty string to use DuckDuckGo fallback)",
)
def discover_search(
    input_path: str | None, output: str | None, n: int, concurrency: int, searxng_url: str
) -> None:
    """Phase 1 of batch enrichment: gather N web searches per app via SearXNG."""
    from .discovery.batch_enrich import run_search

    inp = Path(input_path) if input_path else DEFAULT_DATA_DIR / "raw_software.jsonl"
    out = Path(output) if output else DEFAULT_DATA_DIR / "search_results.jsonl"

    if not inp.exists():
        click.echo(f"Input not found: {inp}. Run 'discover software' first.", err=True)
        raise SystemExit(1)

    enriched = DEFAULT_DATA_DIR / "enriched_software.jsonl"
    brave_key = os.environ.get("BRAVE_API_KEY")
    provider = (searxng_url or "duckduckgo") + (" + brave api" if brave_key else "")
    click.echo(
        f"Gathering {n} searches per app from {inp} (concurrency={concurrency}, provider={provider})"
    )
    asyncio.run(
        run_search(
            inp,
            out,
            n=n,
            concurrency=concurrency,
            enriched_path=enriched,
            searxng_url=searxng_url or None,
            brave_api_key=brave_key,
        )
    )
    click.echo(f"Search results written to {out}")


@discover.command("batch-submit")
@click.option(
    "--input", "input_path", type=click.Path(exists=True), default=None, help="search_results.jsonl"
)
@click.option("--bucket", required=True, help="S3 bucket for input/output")
@click.option("--prefix", default="cua-sandbox-apps/enrich", help="S3 key prefix")
@click.option(
    "--model", default="us.anthropic.claude-haiku-4-5-20251001-v1:0", help="Bedrock model ID"
)
@click.option("--region", default="us-east-1")
@click.option("--role-arn", default=None, help="IAM role ARN for Bedrock (if needed)")
def discover_batch_submit(
    input_path: str | None, bucket: str, prefix: str, model: str, region: str, role_arn: str | None
) -> None:
    """Phase 2: upload search results to S3 and create Bedrock batch inference job."""
    from .discovery.batch_enrich import submit_batch

    inp = Path(input_path) if input_path else DEFAULT_DATA_DIR / "search_results.jsonl"

    if not inp.exists():
        click.echo(f"Input not found: {inp}. Run 'discover search' first.", err=True)
        raise SystemExit(1)

    enriched = DEFAULT_DATA_DIR / "enriched_software.jsonl"
    job_arn = submit_batch(
        inp,
        s3_bucket=bucket,
        s3_prefix=prefix,
        model_id=model,
        region=region,
        role_arn=role_arn,
        enriched_path=enriched,
    )
    click.echo(f"Job ARN: {job_arn}")
    click.echo(f"Run 'discover batch-fetch --job-arn {job_arn}' when complete.")


@discover.command("batch-fetch")
@click.option("--job-arn", required=True, help="Bedrock batch job ARN")
@click.option("--output", type=click.Path(), default=None)
@click.option("--region", default="us-east-1")
@click.option(
    "--no-poll", is_flag=True, help="Don't wait — fetch immediately (job must already be complete)"
)
@click.option("--poll-interval", type=int, default=60, help="Seconds between status checks")
def discover_batch_fetch(
    job_arn: str, output: str | None, region: str, no_poll: bool, poll_interval: int
) -> None:
    """Phase 3: download Bedrock batch results from S3, write enriched JSONL."""
    from .discovery.batch_enrich import fetch_results

    out = Path(output) if output else DEFAULT_DATA_DIR / "enriched_software.jsonl"

    click.echo(f"Fetching results for job {job_arn} → {out}")
    count = fetch_results(
        job_arn, out, region=region, poll=not no_poll, poll_interval=poll_interval
    )
    click.echo(f"Done: {count} entries written to {out}")


@discover.command("dedup")
@click.option("--input", "input_path", type=click.Path(exists=True), default=None)
@click.option("--output", type=click.Path(), default=None)
@click.option("--threshold", type=int, default=88, help="Fuzzy match threshold (0-100)")
def discover_dedup(input_path: str | None, output: str | None, threshold: int) -> None:
    """Deduplicate enriched software catalog."""
    from .discovery.dedup import deduplicate

    inp = Path(input_path) if input_path else DEFAULT_DATA_DIR / "enriched_software.jsonl"
    out = Path(output) if output else DEFAULT_DATA_DIR / "catalog.jsonl"

    if not inp.exists():
        click.echo(f"Input not found: {inp}. Run 'discover enrich' first.", err=True)
        raise SystemExit(1)

    count = deduplicate(inp, out, similarity_threshold=threshold)
    click.echo(f"Catalog: {count} unique apps written to {out}")


@cli.command("generate")
@click.option("--catalog", type=click.Path(exists=True), default=None, help="Catalog JSONL")
@click.option("--apps-dir", type=click.Path(), default=None, help="Output apps directory")
@click.option("--os", "os_filter", multiple=True, help="Only these OS types (repeatable)")
@click.option("--concurrency", type=int, default=4, help="Max concurrent agents")
@click.option("--limit", type=int, default=None, help="Max app/os pairs to process")
@click.option(
    "--strict-install-verify/--no-strict-install-verify",
    default=True,
    help="Pre-submit: bash -n parse + re-run install.sh end-to-end in a fresh sandbox",
)
def generate(
    catalog: str | None,
    apps_dir: str | None,
    os_filter: tuple[str, ...],
    concurrency: int,
    limit: int | None,
    strict_install_verify: bool,
) -> None:
    """Phase 2: Generate install scripts for cataloged apps using Claude agents."""
    from .pipeline.orchestrator import run_orchestrator

    cat = Path(catalog) if catalog else DEFAULT_DATA_DIR / "catalog.jsonl"
    # Fall back to raw_software.jsonl if catalog.jsonl doesn't exist
    if not cat.exists():
        cat = DEFAULT_DATA_DIR / "raw_software.jsonl"
    apps = Path(apps_dir) if apps_dir else Path(__file__).parent / "apps"

    if not cat.exists():
        click.echo(f"Catalog not found: {cat}. Run discovery first.", err=True)
        raise SystemExit(1)

    os_list = list(os_filter) if os_filter else None
    click.echo(
        f"Generating installers: catalog={cat}, apps_dir={apps}, os={os_list or 'auto-detect'}, concurrency={concurrency}"
    )
    asyncio.run(
        run_orchestrator(
            cat,
            apps,
            os_filter=os_list,
            concurrency=concurrency,
            limit=limit,
            strict_install_verify=strict_install_verify,
        )
    )


@cli.command("create-tasks")
@click.option(
    "--apps-dir", type=click.Path(), default=None, help="Apps directory (default: package apps/)"
)
@click.option("--concurrency", type=int, default=4, help="Max concurrent task-creator agents")
@click.option("--limit", type=int, default=None, help="Max (app, os) pairs to process")
@click.option(
    "--seeds",
    "target_seed_count",
    type=int,
    default=100,
    help="Target high-quality seed tasks per app",
)
@click.option("--model", type=str, default="sonnet", help="Claude model for the task creator")
@click.option("--app", "only_apps", multiple=True, help="Only run on these app ids (repeatable)")
def create_tasks(
    apps_dir: str | None,
    concurrency: int,
    limit: int | None,
    target_seed_count: int,
    model: str,
    only_apps: tuple[str, ...],
) -> None:
    """Phase 3: Generate open-ended deterministic tasks per app (resumable).

    Runs after `generate`. For each (app, os) pair with a working install,
    spins up the app, researches real-world usage (guides, freelancer listings,
    sample data, parser/simulator libs, git repos), explores the UI safely,
    and writes a small set of seed tasks audited by an independent verifier.
    """
    from .pipeline.tasks.orchestrator import run_task_orchestrator

    apps = Path(apps_dir) if apps_dir else Path(__file__).parent / "apps"
    if not apps.exists():
        click.echo(f"Apps dir not found: {apps}. Run 'generate' first.", err=True)
        raise SystemExit(1)
    click.echo(
        f"Creating tasks: apps_dir={apps}, concurrency={concurrency}, seeds={target_seed_count}, model={model}"
    )
    asyncio.run(
        run_task_orchestrator(
            apps,
            concurrency=concurrency,
            limit=limit,
            target_seed_count=target_seed_count,
            model=model,
            only_apps=list(only_apps) if only_apps else None,
        )
    )


@cli.command("extract-tools")
@click.option("--apps-dir", type=click.Path(), default=None)
@click.option("--tasks-dir", type=click.Path(), default=None)
@click.option("--app", "only_apps", multiple=True, required=True, help="App ids (repeatable)")
@click.option("--concurrency", type=int, default=2)
@click.option("--tools-per-app", type=int, default=25)
@click.option("--model", type=str, default="sonnet")
def extract_tools(apps_dir, tasks_dir, only_apps, concurrency, tools_per_app, model):
    """Stage 1: Extract MCP tool primitives per app."""
    from .pipeline.tasks.mcp.tool_extractor import extract_tools_for_app

    apps = Path(apps_dir) if apps_dir else Path(__file__).parent / "apps"
    tasks = Path(tasks_dir) if tasks_dir else Path(__file__).parent / "tasks"

    async def _run():
        import asyncio

        sem = asyncio.Semaphore(concurrency)

        async def _one(app_id):
            async with sem:
                app_dir = apps / app_id
                if not app_dir.exists():
                    # App has no install dir — create a minimal stub so the
                    # extractor can still run in research-only mode.
                    app_dir.mkdir(parents=True, exist_ok=True)
                    click.echo(f"No install dir for {app_id} — running research-only")
                await extract_tools_for_app(
                    app_dir, "linux", target_tool_count=tools_per_app, model=model, tasks_root=tasks
                )

        await asyncio.gather(*[_one(a) for a in only_apps])

    asyncio.run(_run())


@cli.command("verify-replica")
@click.argument("app_id")
@click.option("--tasks-dir", type=click.Path(), default=None)
@click.option(
    "--replica-source", type=click.Path(), default=None, help="Local path to replica source code"
)
@click.option("--os", "os_type", default="linux")
@click.option("--model", type=str, default="sonnet")
def verify_replica(
    app_id: str, tasks_dir: str | None, replica_source: str | None, os_type: str, model: str
) -> None:
    """Verify a replica environment: boot sandbox, install, screenshot, check /gym.

    The agent can debug failures and edit source code — patches are saved for review.
    Reads config from tasks/{app_id}/{os}/replica/app.json.
    """
    from .pipeline.replica.creator import create_replica_installer

    tasks = Path(tasks_dir) if tasks_dir else Path(__file__).parent / "tasks"
    source = Path(replica_source) if replica_source else None
    click.echo(f"Verifying replica: {app_id}/{os_type}, tasks_dir={tasks}")
    result = asyncio.run(
        create_replica_installer(
            app_id,
            os_type,
            tasks_root=tasks,
            replica_source_dir=source,
            model=model,
        )
    )
    if result and result.get("verification_passed"):
        click.echo(f"PASSED: {app_id} replica verified")
        if result.get("patch_file"):
            click.echo(f"Patches saved: {result['patch_file']}")
    else:
        click.echo(f"FAILED: {app_id} replica verification failed", err=True)
        if result:
            for k, v in result.get("checks", {}).items():
                click.echo(f"  {k}: {v}", err=True)
        raise SystemExit(1)


@cli.command("compose-workflows")
@click.option("--tasks-dir", type=click.Path(), default=None)
@click.option(
    "--app", "app_ids", multiple=True, required=True, help="App ids whose tools to compose"
)
@click.option("--target", type=int, default=50)
@click.option("--model", type=str, default="sonnet")
def compose_workflows(tasks_dir, app_ids, target, model):
    """Stage 2: Compose multi-app workflows from extracted MCP tools."""
    from .pipeline.tasks.mcp.workflow_composer import compose_workflows as _compose

    tasks = Path(tasks_dir) if tasks_dir else Path(__file__).parent / "tasks"
    asyncio.run(_compose(list(app_ids), tasks, target_count=target, model=model))


@cli.command("amplify-tasks")
@click.argument("app_id")
@click.option("--tasks-dir", type=click.Path(), default=None)
@click.option("--os", "os_type", default="linux")
@click.option("--target", type=int, default=75, help="Target amplified task count")
@click.option("--model", type=str, default="us.anthropic.claude-haiku-4-5-20251001-v1:0")
def amplify_tasks_cmd(
    app_id: str, tasks_dir: str | None, os_type: str, target: int, model: str
) -> None:
    """Amplify seed tasks using a cheap non-agentic LLM (Gym-Anything §4).

    Reads seeds from tasks/{app}/{os}/tasks.jsonl, outputs to tasks_amplified.jsonl.
    """
    from .pipeline.tasks.amplifier import amplify_tasks

    tasks = Path(tasks_dir) if tasks_dir else Path(__file__).parent / "tasks"
    result = amplify_tasks(app_id, os_type, tasks_root=tasks, target_count=target, model_id=model)
    click.echo(f"Amplified {len(result)} tasks for {app_id}")


@cli.command("amplify-tasks-agent")
@click.argument("app_id")
@click.option("--tasks-dir", type=click.Path(), default=None)
@click.option("--os", "os_type", default="linux")
@click.option("--target", type=int, default=75)
@click.option("--model", type=str, default="sonnet")
def amplify_tasks_agent_cmd(
    app_id: str, tasks_dir: str | None, os_type: str, target: int, model: str
) -> None:
    """Agent-driven task amplification with sandbox validation (no reviser needed)."""
    from .pipeline.tasks.amplifier_agent import amplify_tasks_agent

    tasks = Path(tasks_dir) if tasks_dir else Path(__file__).parent / "tasks"
    result = asyncio.run(
        amplify_tasks_agent(app_id, os_type, tasks_root=tasks, target_count=target, model=model)
    )
    click.echo(f"Amplified {len(result)} validated tasks for {app_id}")


@cli.command("amplify-workflows")
@click.option("--tasks-dir", type=click.Path(), default=None)
@click.option("--target", type=int, default=50, help="Target amplified workflow count")
@click.option("--model", type=str, default="us.anthropic.claude-haiku-4-5-20251001-v1:0")
def amplify_workflows_cmd(tasks_dir: str | None, target: int, model: str) -> None:
    """Amplify seed workflows using a cheap non-agentic LLM (Gym-Anything §4)."""
    from .pipeline.tasks.amplifier import amplify_workflows

    tasks = Path(tasks_dir) if tasks_dir else Path(__file__).parent / "tasks"
    result = amplify_workflows(tasks_root=tasks, target_count=target, model_id=model)
    click.echo(f"Amplified {len(result)} workflows")


@cli.command("revise-tasks")
@click.argument("app_id")
@click.option("--tasks-dir", type=click.Path(), default=None)
@click.option("--os", "os_type", default="linux")
@click.option("--type", "file_type", type=click.Choice(["seed", "amplified"]), default="seed")
@click.option("--max-retries", type=int, default=3)
def revise_tasks_cmd(
    app_id: str, tasks_dir: str | None, os_type: str, file_type: str, max_retries: int
) -> None:
    """Revise tasks to fix quality gaps (missing fields, read-only, bad scripts)."""
    from .pipeline.tasks.reviser import revise_tasks

    tasks = Path(tasks_dir) if tasks_dir else Path(__file__).parent / "tasks"
    result = revise_tasks(
        app_id, os_type, tasks_root=tasks, file_type=file_type, max_retries=max_retries
    )
    click.echo(f"Revised: {result['revised']}, OK: {result['skipped']}, Failed: {result['failed']}")


@cli.command("revise-tasks-agent")
@click.argument("app_id")
@click.option("--tasks-dir", type=click.Path(), default=None)
@click.option("--os", "os_type", default="linux")
@click.option("--type", "file_type", type=click.Choice(["seed", "amplified"]), default="seed")
@click.option("--model", type=str, default="sonnet")
def revise_tasks_agent_cmd(
    app_id: str, tasks_dir: str | None, os_type: str, file_type: str, model: str
) -> None:
    """Agent-driven task revision: boots sandbox, validates start+end state, fixes issues, produces patches."""
    from .pipeline.tasks.reviser_agent import revise_tasks_agent

    tasks = Path(tasks_dir) if tasks_dir else Path(__file__).parent / "tasks"
    result = asyncio.run(
        revise_tasks_agent(app_id, os_type, tasks_root=tasks, file_type=file_type, model=model)
    )
    if "error" in result:
        click.echo(f"Error: {result['error']}", err=True)
        raise SystemExit(1)
    click.echo(f"Total: {result['total']}, Passed: {result['passed']}, Failed: {result['failed']}")


@cli.command("revise-workflows-agent")
@click.option("--tasks-dir", type=click.Path(), default=None)
@click.option("--type", "file_type", type=click.Choice(["seed", "amplified"]), default="seed")
@click.option("--model", type=str, default="sonnet")
def revise_workflows_agent_cmd(tasks_dir: str | None, file_type: str, model: str) -> None:
    """Agent-driven workflow revision. Safe to run N instances in parallel — uses file locks."""
    from .pipeline.tasks.reviser_agent import revise_workflows_agent

    tasks = Path(tasks_dir) if tasks_dir else Path(__file__).parent / "tasks"
    result = asyncio.run(revise_workflows_agent(tasks_root=tasks, file_type=file_type, model=model))
    if "error" in result:
        click.echo(f"Error: {result['error']}", err=True)
        raise SystemExit(1)
    click.echo(
        f"This agent: {result['total']} processed, {result['passed']} passed. "
        f"Global: {result.get('global_done', '?')}/{result.get('global_total', '?')} done."
    )


@cli.command("revise-workflows")
@click.option("--tasks-dir", type=click.Path(), default=None)
@click.option("--type", "file_type", type=click.Choice(["seed", "amplified"]), default="seed")
@click.option("--max-retries", type=int, default=3)
def revise_workflows_cmd(tasks_dir: str | None, file_type: str, max_retries: int) -> None:
    """Revise workflows to fix quality gaps (descriptive oracles, missing fields)."""
    from .pipeline.tasks.reviser import revise_workflows

    tasks = Path(tasks_dir) if tasks_dir else Path(__file__).parent / "tasks"
    result = revise_workflows(tasks_root=tasks, file_type=file_type, max_retries=max_retries)
    click.echo(f"Revised: {result['revised']}, OK: {result['skipped']}, Failed: {result['failed']}")


@cli.command("validate-tasks")
@click.argument("app_id")
@click.option("--tasks-dir", type=click.Path(), default=None)
@click.option("--os", "os_type", default="linux")
@click.option("--type", "file_type", type=click.Choice(["seed", "amplified"]), default="seed")
def validate_tasks_cmd(app_id: str, tasks_dir: str | None, os_type: str, file_type: str) -> None:
    """E2E validate tasks: boot sandbox, run setup, execute golden path, verify extract."""
    from .pipeline.tasks.validator import validate_tasks_batch

    tasks = Path(tasks_dir) if tasks_dir else Path(__file__).parent / "tasks"
    result = asyncio.run(
        validate_tasks_batch(app_id, os_type, tasks_root=tasks, file_type=file_type)
    )
    if "error" in result:
        click.echo(f"Error: {result['error']}", err=True)
        raise SystemExit(1)
    click.echo(
        f"Total: {result['total']}, Start OK: {result['start_ok']}, End OK: {result['end_ok']}, Failed: {result['failed']}"
    )
    for r in result.get("results", []):
        if r.get("issues"):
            click.echo(f"  {r['task_id']}: {r['issues']}")


@cli.command("review-task")
@click.argument("app_id")
@click.argument("task_id")
@click.option("--os", "os_type", default="linux")
@click.option("--apps-dir", type=click.Path(), default=None)
@click.option("--tasks-dir", type=click.Path(), default=None)
@click.option("--port", type=int, default=8787)
def review_task(
    app_id: str, task_id: str, os_type: str, apps_dir: str | None, tasks_dir: str | None, port: int
) -> None:
    """Boot a task in a sandbox with VNC for human review."""
    from .pipeline.tasks.review.server import review_task as _review

    apps = Path(apps_dir) if apps_dir else Path(__file__).parent / "apps"
    tasks = Path(tasks_dir) if tasks_dir else Path(__file__).parent / "tasks"
    asyncio.run(_review(app_id, task_id, apps, tasks, target_os=os_type, port=port))


@cli.command("verify")
@click.argument("app_id")
@click.option("--os", "os_type", default=None, help="OS to verify (default: all available)")
@click.option("--apps-dir", type=click.Path(), default=None)
def verify(app_id: str, os_type: str | None, apps_dir: str | None) -> None:
    """Run the pytest verification for an app's install script."""
    import subprocess

    apps = Path(apps_dir) if apps_dir else Path(__file__).parent / "apps"
    app_dir = apps / app_id

    if not app_dir.exists():
        click.echo(f"App not found: {app_dir}", err=True)
        raise SystemExit(1)

    os_dirs = [app_dir / os_type] if os_type else [d for d in app_dir.iterdir() if d.is_dir()]

    for od in os_dirs:
        test_file = od / "test_install.py"
        if not test_file.exists():
            click.echo(f"  {od.name}: no test_install.py, skipping")
            continue
        click.echo(f"  {od.name}: running pytest...")
        result = subprocess.run(
            ["python", "-m", "pytest", str(test_file), "-v"],
            cwd=str(od),
        )
        if result.returncode == 0:
            click.echo(f"  {od.name}: PASSED")
        else:
            click.echo(f"  {od.name}: FAILED (exit {result.returncode})")


if __name__ == "__main__":
    cli()
