"""Install command - Install an environment."""

import shutil
from pathlib import Path

RESET = "\033[0m"
BOLD = "\033[1m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"

def execute(args):
    """Execute the install command.
    
    This command validates and prepares an environment for use.
    """
    env_path = Path(args.env_path)
    
    # Check if environment exists
    if not env_path.exists():
        print(f"{RED}Error: Environment not found: {env_path}{RESET}")
        return 1
    
    # Check for main.py
    main_file = env_path / "main.py"
    if not main_file.exists():
        print(f"{RED}Error: main.py not found in {env_path}{RESET}")
        return 1
    
    # Validate the environment by trying to load it
    try:
        from cua_bench import make
        env = make(str(env_path))
        
        # Check for required decorators
        if env.tasks_config_fn is None:
            print(f"{YELLOW}Warning: No @cb.tasks_config function found in {main_file}{RESET}")
        if env.setup_task_fn is None:
            print(f"{YELLOW}Warning: No @cb.setup_task function found in {main_file}{RESET}")
        
        # Try to load tasks
        if env.tasks_config_fn:
            tasks = env.tasks_config_fn()
            print(f"{GREEN}✓ Environment validated: {env_path}{RESET}")
            print(f"  {GREY}Found:{RESET} {len(tasks)} task(s)")
            for i, task in enumerate(tasks):
                print(f"    {GREY}Task {i}:{RESET} {task.description}")
        else:
            print(f"{GREEN}✓ Environment found: {env_path}{RESET}")
        
        env.close()
        
    except Exception as e:
        print(f"{RED}Error validating environment: {e}{RESET}")
        return 1
    
    print(f"\n{CYAN}Environment '{env_path.name}' is ready to use!{RESET}")
    print(f"  {GREY}Run a task:{RESET}  cb run {env_path}")
    print(f"  {GREY}Batch solve:{RESET} cb batch solve {env_path}")
    
    return 0
