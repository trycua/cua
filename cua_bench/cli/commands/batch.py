"""Batch command - Run batch processing on GCP."""

import asyncio
import time
from pathlib import Path

RESET = "\033[0m"
BOLD = "\033[1m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"

def execute(args):
    """Execute the batch command."""
    env_path = Path(args.env_path)
    
    if not env_path.exists():
        print(f"{RED}Error: Environment not found: {env_path}{RESET}")
        return 1
    
    # Validate environment first
    try:
        from cua_bench import make
        env = make(str(env_path))
        
        # Load tasks to get count
        if env.tasks_config_fn is None:
            print(f"{RED}Error: No @cb.tasks_config function found{RESET}")
            return 1
        
        tasks = env.tasks_config_fn()
        task_count = len(tasks)
        print(f"{CYAN}Found {BOLD}{task_count}{RESET}{CYAN} task(s) in environment{RESET}")
        
        env.close()
        
    except Exception as e:
        print(f"{RED}Error validating environment: {e}{RESET}")
        return 1
    
    # Run batch processing (always solve mode now)
    return asyncio.run(run_batch_solve(args, env_path, task_count))


async def run_batch_solve(args, env_path: Path, task_count: int):
    """Run batch solve on GCP."""
    from cua_bench.batch import execute_batch
    import tempfile
    
    # Generate job name if not provided
    job_name = args.job_name
    if job_name is None:
        job_name = f"td-{env_path.name}-{int(time.time())}"
    
    # Check if dump mode
    dump_mode = getattr(args, 'dump_mode', False)
    
    # Use specified task count or default to all tasks
    num_tasks = args.tasks if args.tasks is not None else task_count
    
    # If --view is specified but no --output-dir, create a temporary directory
    output_dir = getattr(args, 'output_dir', None)
    if getattr(args, 'view', False) and output_dir is None:
        output_dir = tempfile.mkdtemp(prefix=f"cb-{job_name}-")
        print(f"  {GREY}Output (temp):{RESET} {output_dir}")
    
    print(f"\n{CYAN}Starting batch job: {BOLD}{job_name}{RESET}")
    print(f"  {GREY}Environment:{RESET} {env_path}")
    print(f"  {GREY}Tasks:{RESET} {num_tasks}")
    print(f"  {GREY}Parallelism:{RESET} {args.parallelism}")
    print(f"  {GREY}Mode:{RESET} {'Local (Docker)' if args.local else 'GCP Batch'}")
    print(f"  {GREY}Type:{RESET} {'Dump (no solver)' if dump_mode else 'Solve'}")
    
    try:
        # Build container script with optional flags
        script_flags = []
        if dump_mode:
            script_flags.append('--dump')
        if getattr(args, 'save_pngs', False):
            script_flags.append('--save-pngs')
        if getattr(args, 'filter', None):
            script_flags.append(f'--filter "{args.filter}"')
        
        container_script = f"python3 -m cua_bench.batch.solver {{env_path}}{' ' + ' '.join(script_flags) if script_flags else ''}"
        
        # Execute batch job
        logs = await execute_batch(
            job_name=job_name,
            env_path=env_path,
            container_script=container_script,
            task_count=num_tasks,
            task_parallelism=args.parallelism,
            run_local=args.local,
            image_uri=args.image,
            output_dir=output_dir,
            auto_cleanup=False,
        )
        
        print(f"\n{GREEN}✓ Batch job completed successfully!{RESET}")
        
        if logs:
            print(f"\n{GREY}Job output:{RESET}")
            for line in logs:
                print(f"  {line}")
        
        # Concatenate datasets if requested (local mode only)
        if getattr(args, 'concatenate', False) and args.local and output_dir:
            print(f"\n{CYAN}Concatenating task datasets...{RESET}")
            try:
                from datasets import load_from_disk, concatenate_datasets
                from pathlib import Path
                
                output_path = Path(output_dir)
                task_datasets = []
                
                # Find all task_*_trace directories
                for task_dir in sorted(output_path.glob("task_*_trace")):
                    if task_dir.is_dir():
                        try:
                            dataset = load_from_disk(str(task_dir))
                            task_datasets.append(dataset)
                            print(f"  {GREY}Loaded:{RESET} {task_dir.name} ({len(dataset)} samples)")
                        except Exception as e:
                            print(f"  {YELLOW}Warning: Failed to load {task_dir.name}: {e}{RESET}")
                
                if task_datasets:
                    # Concatenate all datasets
                    combined_dataset = concatenate_datasets(task_datasets)
                    
                    # Save concatenated dataset
                    combined_output = output_path / "task_traces"
                    combined_dataset.save_to_disk(str(combined_output))
                    
                    print(f"  {GREEN}✓ Concatenated {len(task_datasets)} datasets into {combined_output}{RESET}")
                    print(f"  {GREY}Total samples:{RESET} {len(combined_dataset)}")
                    
                    # Delete individual task directories after successful concatenation
                    import shutil
                    deleted_count = 0
                    for task_dir in sorted(output_path.glob("task_*_trace")):
                        if task_dir.is_dir():
                            try:
                                shutil.rmtree(str(task_dir))
                                deleted_count += 1
                            except Exception as e:
                                print(f"  {YELLOW}Warning: Failed to delete {task_dir.name}: {e}{RESET}")
                    
                    if deleted_count > 0:
                        print(f"  {GREY}Cleaned up {deleted_count} individual task directories{RESET}")
                else:
                    print(f"  {YELLOW}Warning: No task datasets found to concatenate{RESET}")
                    
            except Exception as e:
                print(f"  {RED}Error during concatenation: {e}{RESET}")
        elif getattr(args, 'concatenate', False) and not args.local:
            print(f"  {YELLOW}Warning: --concatenate only works in local mode{RESET}")
        
        # Optionally open multi-trace viewer
        if output_dir:
            if getattr(args, 'view', False):
                try:
                    from types import SimpleNamespace
                    from . import view_traces as _view_traces
                    _view_traces.execute(SimpleNamespace(outputs=str(output_dir)))
                except Exception:
                    # Fall back to printing suggestion
                    print(f"\n{GREY}You can view results with:{RESET}\n  cb view-traces {output_dir}")
            else:
                print(f"\n{GREY}Tip: view results with:{RESET}\n  cb view-traces {output_dir}")
        
        return 0
        
    except Exception as e:
        print(f"\n{RED}✗ Batch job failed: {e}{RESET}")
        import traceback
        traceback.print_exc()
        return 1
