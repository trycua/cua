"""Skills management commands for CUA CLI.

Skills are recorded demonstrations that can guide agent behavior.
Each skill contains:
- SKILL.md: Markdown file with frontmatter and steps
- trajectory/: Directory with video, events.json, trajectory.json, screenshots
"""

import argparse
import json
import shutil
import webbrowser
from datetime import datetime
from pathlib import Path
from typing import Any, Optional

from cua_cli.utils.async_utils import run_async
from cua_cli.utils.output import (
    print_error,
    print_info,
    print_json,
    print_success,
    print_table,
)

# Skills directory
SKILLS_DIR = Path.home() / ".cua" / "skills"


def register_parser(subparsers: argparse._SubParsersAction) -> None:
    """Register the skills command and subcommands."""
    skills_parser = subparsers.add_parser(
        "skills",
        help="Manage demonstration skills",
        description="Record and manage demonstration skills for agent guidance",
    )

    skills_subparsers = skills_parser.add_subparsers(
        dest="skills_command",
        help="Skills command",
    )

    # list command
    list_parser = skills_subparsers.add_parser(
        "list",
        aliases=["ls"],
        help="List all saved skills",
    )
    list_parser.add_argument(
        "--json",
        action="store_true",
        help="Output as JSON",
    )

    # read command
    read_parser = skills_subparsers.add_parser(
        "read",
        help="Read a skill",
    )
    read_parser.add_argument(
        "name",
        help="Skill name",
    )
    read_parser.add_argument(
        "--format",
        "-f",
        choices=["json", "md"],
        default="md",
        help="Output format (default: md)",
    )

    # replay command
    replay_parser = skills_subparsers.add_parser(
        "replay",
        help="Open the video recording for a skill",
    )
    replay_parser.add_argument(
        "name",
        help="Skill name",
    )

    # delete command
    delete_parser = skills_subparsers.add_parser(
        "delete",
        help="Delete a skill",
    )
    delete_parser.add_argument(
        "name",
        help="Skill name",
    )

    # clean command
    skills_subparsers.add_parser(
        "clean",
        help="Delete all skills (with confirmation)",
    )

    # record command
    record_parser = skills_subparsers.add_parser(
        "record",
        help="Record a demonstration and create a skill",
    )
    record_parser.add_argument(
        "--sandbox",
        "-s",
        type=str,
        help="Sandbox name to connect to",
    )
    record_parser.add_argument(
        "--vnc-url",
        "-u",
        type=str,
        help="Direct VNC URL to connect to",
    )
    record_parser.add_argument(
        "--provider",
        "-p",
        choices=["anthropic", "openai"],
        default="anthropic",
        help="LLM provider for captioning (default: anthropic)",
    )
    record_parser.add_argument(
        "--model",
        "-m",
        type=str,
        help="Model to use for captioning",
    )
    record_parser.add_argument(
        "--api-key",
        "-k",
        type=str,
        help="API key for the LLM provider",
    )
    record_parser.add_argument(
        "--name",
        "-n",
        type=str,
        help="Skill name (skips interactive prompt)",
    )
    record_parser.add_argument(
        "--description",
        "-d",
        type=str,
        help="Skill description (skips interactive prompt)",
    )


def execute(args: argparse.Namespace) -> int:
    """Execute skills command based on subcommand."""
    cmd = getattr(args, "skills_command", None)

    if cmd in ("list", "ls"):
        return cmd_list(args)
    elif cmd == "read":
        return cmd_read(args)
    elif cmd == "replay":
        return cmd_replay(args)
    elif cmd == "delete":
        return cmd_delete(args)
    elif cmd == "clean":
        return cmd_clean(args)
    elif cmd == "record":
        return cmd_record(args)
    else:
        print_error("Usage: cua skills <command>")
        print_info("Commands: list, read, replay, delete, clean, record")
        return 1


def _ensure_skills_dir() -> None:
    """Ensure skills directory exists."""
    SKILLS_DIR.mkdir(parents=True, exist_ok=True)


def _parse_frontmatter(content: str) -> Optional[dict[str, str]]:
    """Parse YAML frontmatter from markdown content."""
    import re

    match = re.match(r"^---\n(.*?)\n---\n(.*)$", content, re.DOTALL)
    if not match:
        return None

    frontmatter = match.group(1)
    body = match.group(2).strip()

    name_match = re.search(r"^name:\s*(.+)$", frontmatter, re.MULTILINE)
    desc_match = re.search(r"^description:\s*(.+)$", frontmatter, re.MULTILINE)

    if not name_match or not desc_match:
        return None

    return {
        "name": name_match.group(1).strip(),
        "description": desc_match.group(1).strip(),
        "body": body,
    }


def _get_skill_info(skill_dir: Path) -> Optional[dict[str, Any]]:
    """Get skill info from a skill directory."""
    skill_path = skill_dir / "SKILL.md"
    if not skill_path.exists():
        return None

    content = skill_path.read_text()
    parsed = _parse_frontmatter(content)
    if not parsed:
        return None

    # Try to read trajectory.json for additional info
    steps = 0
    created = None
    trajectory_path = skill_dir / "trajectory" / "trajectory.json"
    if trajectory_path.exists():
        try:
            traj_data = json.loads(trajectory_path.read_text())
            steps = len(traj_data.get("trajectory", []))
            if traj_data.get("metadata", {}).get("created_at"):
                created = traj_data["metadata"]["created_at"]
        except Exception:
            pass

    return {
        "name": parsed["name"],
        "description": parsed["description"],
        "steps": steps,
        "created": created,
        "path": str(skill_dir),
    }


def cmd_list(args: argparse.Namespace) -> int:
    """List all skills."""
    _ensure_skills_dir()

    skills = []
    for skill_dir in sorted(SKILLS_DIR.iterdir()):
        if not skill_dir.is_dir():
            continue
        info = _get_skill_info(skill_dir)
        if info:
            skills.append(info)

    if args.json:
        print_json(skills)
        return 0

    if not skills:
        print_info("No skills found.")
        print_info("Record a skill with: cua skills record --sandbox <name>")
        return 0

    # Format for table
    rows = []
    for skill in skills:
        created = "-"
        if skill["created"]:
            try:
                dt = datetime.fromisoformat(skill["created"].replace("Z", "+00:00"))
                created = dt.strftime("%Y-%m-%d")
            except Exception:
                created = skill["created"][:10]

        rows.append(
            {
                "name": skill["name"],
                "description": skill["description"][:40]
                + ("..." if len(skill["description"]) > 40 else ""),
                "steps": str(skill["steps"]),
                "created": created,
            }
        )

    columns = [
        ("name", "NAME"),
        ("description", "DESCRIPTION"),
        ("steps", "STEPS"),
        ("created", "CREATED"),
    ]
    print_table(rows, columns)
    return 0


def cmd_read(args: argparse.Namespace) -> int:
    """Read a skill."""
    _ensure_skills_dir()

    skill_dir = SKILLS_DIR / args.name
    skill_path = skill_dir / "SKILL.md"

    if not skill_path.exists():
        print_error(f"Skill not found: {args.name}")
        return 1

    content = skill_path.read_text()

    if args.format == "md":
        print(content)
        return 0

    # JSON format - include trajectory data
    parsed = _parse_frontmatter(content)
    if not parsed:
        print_error(f"Invalid skill file format: {args.name}")
        return 1

    trajectory_path = skill_dir / "trajectory" / "trajectory.json"
    trajectory = []
    metadata = {}

    if trajectory_path.exists():
        try:
            traj_data = json.loads(trajectory_path.read_text())
            trajectory = traj_data.get("trajectory", [])
            metadata = traj_data.get("metadata", {})
        except Exception as e:
            print_error(f"Failed to read trajectory: {e}")

    result = {
        "name": parsed["name"],
        "description": parsed["description"],
        "trajectory": trajectory,
        "skill_prompt": parsed["body"],
        "trajectory_dir": str(skill_dir / "trajectory"),
        "metadata": metadata,
    }

    print_json(result)
    return 0


def cmd_replay(args: argparse.Namespace) -> int:
    """Open the video recording for a skill."""
    _ensure_skills_dir()

    skill_dir = SKILLS_DIR / args.name
    if not skill_dir.exists():
        print_error(f"Skill not found: {args.name}")
        return 1

    # Find MP4 file
    trajectory_dir = skill_dir / "trajectory"
    mp4_files = list(trajectory_dir.glob("*.mp4"))

    if not mp4_files:
        print_error(f"No video found in: {trajectory_dir}")
        return 1

    video_path = mp4_files[0]
    print_info(f"Opening: {video_path}")
    webbrowser.open(f"file://{video_path}")
    return 0


def cmd_delete(args: argparse.Namespace) -> int:
    """Delete a skill."""
    _ensure_skills_dir()

    skill_dir = SKILLS_DIR / args.name
    if not skill_dir.exists():
        print_error(f"Skill not found: {args.name}")
        return 1

    shutil.rmtree(skill_dir)
    print_success(f"Deleted skill: {args.name}")
    return 0


def cmd_clean(args: argparse.Namespace) -> int:
    """Delete all skills with confirmation."""
    _ensure_skills_dir()

    skills = [d for d in SKILLS_DIR.iterdir() if d.is_dir() and (d / "SKILL.md").exists()]

    if not skills:
        print_info("No skills to clean.")
        return 0

    print_info("Skills to delete:")
    for skill_dir in sorted(skills):
        print(f"  - {skill_dir.name}")

    response = input(f"\nDelete {len(skills)} skill(s)? [y/N]: ").strip().lower()
    if response != "y":
        print_info("Cancelled.")
        return 0

    for skill_dir in skills:
        shutil.rmtree(skill_dir)

    print_success(f"Deleted {len(skills)} skill(s).")
    return 0


def cmd_record(args: argparse.Namespace) -> int:
    """Record a demonstration and create a skill.

    This is a complex operation that:
    1. Starts a WebSocket server to receive the recording
    2. Opens the VNC viewer with recording parameters
    3. Waits for the recording to complete
    4. Extracts frames and captions them with LLM
    5. Saves the skill to disk
    """
    # Check for required dependencies
    if not _check_ffmpeg():
        print_error("ffmpeg is required for skill recording.")
        print_info("Install with: brew install ffmpeg (macOS) or apt install ffmpeg (Linux)")
        return 1

    if not args.sandbox and not args.vnc_url:
        print_error("Either --sandbox or --vnc-url is required")
        return 1

    # Defer to async implementation
    return run_async(_record_skill_async(args))


def _check_ffmpeg() -> bool:
    """Check if ffmpeg is available."""
    import subprocess

    try:
        result = subprocess.run(["which", "ffmpeg"], capture_output=True)
        return result.returncode == 0
    except Exception:
        return False


async def _record_skill_async(args: argparse.Namespace) -> int:
    """Async implementation of skill recording."""
    import asyncio
    import os

    import websockets

    # Get LLM API key
    provider = args.provider
    api_key = args.api_key

    if not api_key:
        if provider == "openai":
            api_key = os.environ.get("OPENAI_API_KEY")
        else:
            api_key = os.environ.get("ANTHROPIC_API_KEY")

    if not api_key:
        env_var = "OPENAI_API_KEY" if provider == "openai" else "ANTHROPIC_API_KEY"
        print_error(f"No {provider.upper()} API key found.")
        print_info(f"Set {env_var} environment variable or use --api-key flag.")
        return 1

    model = args.model
    if not model:
        model = "gpt-4o-mini" if provider == "openai" else "claude-haiku-4-5"

    # Start WebSocket server to receive recording
    recording_data = bytearray()
    recording_complete = asyncio.Event()

    async def handle_ws(websocket):
        nonlocal recording_data
        try:
            async for message in websocket:
                if isinstance(message, bytes):
                    recording_data.extend(message)
        except websockets.exceptions.ConnectionClosed:
            pass
        finally:
            recording_complete.set()

    # Find available port
    import socket

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.bind(("localhost", 0))
    port = sock.getsockname()[1]
    sock.close()

    server = await websockets.serve(handle_ws, "localhost", port)
    record_url = f"ws://localhost:{port}"

    print_info(f"Recording server started on port {port}")

    # Build VNC URL with recording parameters
    if args.sandbox:
        # Get sandbox VNC URL
        from computer.providers.cloud.provider import CloudProvider
        from cua_cli.auth.store import require_api_key

        cloud_api_key = require_api_key()
        provider_inst = CloudProvider(api_key=cloud_api_key)

        async with provider_inst:
            vms = await provider_inst.list_vms()
            sandbox = next((vm for vm in vms if vm.get("name") == args.sandbox), None)

            if not sandbox:
                print_error(f"Sandbox not found: {args.sandbox}")
                server.close()
                return 1

            if sandbox.get("status") != "running":
                print_error(f"Sandbox is not running (status: {sandbox.get('status')})")
                server.close()
                return 1

            host = sandbox.get("host", f"{args.sandbox}.sandbox.cua.ai")
            password = sandbox.get("password", "")
            from urllib.parse import quote

            base_url = (
                f"https://{host}/vnc.html?autoconnect=true&password={quote(password)}&show_dot=true"
            )
    else:
        base_url = args.vnc_url

    # Add recording parameters
    from urllib.parse import parse_qs, urlencode, urlparse

    parsed = urlparse(base_url)
    params = parse_qs(parsed.query)
    params["autorecord"] = ["true"]
    params["record_format"] = ["mp4"]
    params["record_url"] = [record_url]
    recording_url = (
        f"{parsed.scheme}://{parsed.netloc}{parsed.path}?{urlencode(params, doseq=True)}"
    )

    print_info("\nRecording will start automatically when you connect.")
    print_info("When finished, click 'Stop Recording' in the VNC panel.\n")

    import webbrowser

    webbrowser.open(recording_url)

    # Wait for recording (30 min timeout)
    try:
        await asyncio.wait_for(recording_complete.wait(), timeout=30 * 60)
    except asyncio.TimeoutError:
        print_error("Recording timeout (30 minutes)")
        server.close()
        return 1

    server.close()

    if len(recording_data) == 0:
        print_error("No recording data received")
        return 1

    print_info(f"Received {len(recording_data)} bytes of recording data")

    # Get skill name
    skill_name = args.name
    if not skill_name:
        skill_name = input("Enter skill name: ").strip()
        while not skill_name or not skill_name.replace("-", "").replace("_", "").isalnum():
            print("Use only letters, numbers, hyphens, and underscores.")
            skill_name = input("Enter skill name: ").strip()

    # Ensure unique name
    _ensure_skills_dir()
    final_name = skill_name
    counter = 1
    while (SKILLS_DIR / final_name).exists():
        final_name = f"{skill_name}-{counter}"
        counter += 1

    if final_name != skill_name:
        print_info(f'Skill "{skill_name}" exists, using "{final_name}"')
    skill_name = final_name

    # Get description
    description = args.description
    if not description:
        description = input("Describe what this skill demonstrates: ").strip()
        while not description:
            print("Description is required.")
            description = input("Describe what this skill demonstrates: ").strip()

    print_info("\nProcessing recording...")

    # Process recording
    result = await _process_recording(
        recording_data=bytes(recording_data),
        skill_name=skill_name,
        description=description,
        provider=provider,
        model=model,
        api_key=api_key,
    )

    if not result:
        print_error("Failed to process recording")
        return 1

    print_success(f"\nSkill saved: {SKILLS_DIR / skill_name / 'SKILL.md'}")
    print_info(f"Steps: {result['steps']}")
    return 0


async def _process_recording(
    recording_data: bytes,
    skill_name: str,
    description: str,
    provider: str,
    model: str,
    api_key: str,
) -> Optional[dict[str, Any]]:
    """Process recording data and create skill files."""
    import struct
    import subprocess
    import tempfile

    # Parse recording format: [4 bytes JSON length][JSON][MP4 data]
    if len(recording_data) < 4:
        print_error("Recording data too short")
        return None

    json_length = struct.unpack(">I", recording_data[:4])[0]
    if len(recording_data) < 4 + json_length:
        print_error("Invalid recording format")
        return None

    json_bytes = recording_data[4 : 4 + json_length]
    mp4_data = recording_data[4 + json_length :]

    if not mp4_data:
        print_error("No video data in recording")
        return None

    try:
        recording_json = json.loads(json_bytes.decode())
    except Exception as e:
        print_error(f"Failed to parse recording JSON: {e}")
        return None

    events = recording_json.get("events", [])
    metadata = recording_json.get("metadata", {})

    # Create skill directory structure
    skill_dir = SKILLS_DIR / skill_name
    trajectory_dir = skill_dir / "trajectory"
    trajectory_dir.mkdir(parents=True, exist_ok=True)

    # Save video
    video_path = trajectory_dir / f"{skill_name}.mp4"
    video_path.write_bytes(mp4_data)

    # Save events
    events_path = trajectory_dir / "events.json"
    events_path.write_text(json.dumps({"events": events, "metadata": metadata}, indent=2))

    # Process each event with LLM captioning
    trajectory = []
    from rich.progress import BarColumn, Progress, SpinnerColumn, TextColumn

    with Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        BarColumn(),
        TextColumn("[progress.percentage]{task.percentage:>3.0f}%"),
    ) as progress:
        task = progress.add_task("Captioning steps...", total=len(events))

        with tempfile.TemporaryDirectory() as temp_dir:
            temp_path = Path(temp_dir)

            for idx, event in enumerate(events):
                step_idx = idx + 1

                # Extract frame at event timestamp
                frame_path = temp_path / f"step_{step_idx}.jpg"
                timestamp_sec = max(0, event.get("timestamp", 0) / 1000 - 0.1)

                result = subprocess.run(
                    [
                        "ffmpeg",
                        "-y",
                        "-ss",
                        f"{timestamp_sec:.3f}",
                        "-i",
                        str(video_path),
                        "-frames:v",
                        "1",
                        "-q:v",
                        "2",
                        str(frame_path),
                    ],
                    capture_output=True,
                )

                if result.returncode != 0 or not frame_path.exists():
                    # Skip if frame extraction fails
                    trajectory.append(
                        {
                            "step_idx": step_idx,
                            "caption": {
                                "observation": "",
                                "think": "",
                                "action": event.get("type", ""),
                                "expectation": "",
                            },
                            "raw_event": event,
                        }
                    )
                    progress.update(task, advance=1)
                    continue

                # Caption with LLM
                caption = await _caption_step(
                    frame_path=frame_path,
                    event=event,
                    step_idx=step_idx,
                    description=description,
                    provider=provider,
                    model=model,
                    api_key=api_key,
                )

                # Save screenshot to trajectory dir
                dest_full = trajectory_dir / f"step_{step_idx}_full.jpg"
                shutil.copy(frame_path, dest_full)

                trajectory.append(
                    {
                        "step_idx": step_idx,
                        "caption": caption,
                        "raw_event": event,
                        "screenshot_full": str(dest_full),
                    }
                )

                progress.update(task, advance=1)

    # Save trajectory
    trajectory_json_path = trajectory_dir / "trajectory.json"
    trajectory_json_path.write_text(
        json.dumps(
            {
                "events": events,
                "trajectory": trajectory,
                "metadata": {
                    "task_description": description,
                    "total_steps": len(trajectory),
                    "width": metadata.get("width"),
                    "height": metadata.get("height"),
                    "duration": metadata.get("duration"),
                    "created_at": datetime.now().isoformat(),
                },
            },
            indent=2,
        )
    )

    # Generate skill markdown
    steps_text = "\n".join(
        [
            f"Step {s['step_idx']}: {s['caption'].get('action', s['raw_event'].get('type', ''))}"
            for s in trajectory
        ]
    )

    skill_prompt = f"""You have been shown a demonstration of how to perform this task:
{description}

The demonstration consisted of the following steps:
{steps_text}

Follow this workflow pattern, adapting as needed for the current screen state.
Total steps: {len(trajectory)}"""

    steps_markdown = "\n".join(
        [
            f"### Step {s['step_idx']}: {s['caption'].get('action', s['raw_event'].get('type', ''))}\n\n"
            f"**Context:** {s['caption'].get('observation', '')}\n\n"
            f"**Intent:** {s['caption'].get('think', '')}\n\n"
            f"**Expected Result:** {s['caption'].get('expectation', '')}\n"
            for s in trajectory
        ]
    )

    skill_content = f"""---
name: {skill_name}
description: {description}
---

# {skill_name}

{description}

## Steps

{steps_markdown}

## Agent Prompt

{skill_prompt}
"""

    skill_path = skill_dir / "SKILL.md"
    skill_path.write_text(skill_content)

    return {"steps": len(trajectory)}


async def _caption_step(
    frame_path: Path,
    event: dict,
    step_idx: int,
    description: str,
    provider: str,
    model: str,
    api_key: str,
) -> dict[str, str]:
    """Caption a single step using LLM."""
    import base64

    import aiohttp

    # Build prompt
    prompt = f"""Describe this GUI action step. The overall task is: {description}

Step {step_idx}: {event.get('type', 'action')}
Event data: {json.dumps(event.get('data', {}))}

Respond with JSON only:
{{
  "Observation": "Describe what you see in the screenshot",
  "Think": "Explain the user's likely intention",
  "Action": "Describe the action being taken",
  "Expectation": "What should happen after this action"
}}"""

    # Read image
    image_data = frame_path.read_bytes()
    image_b64 = base64.b64encode(image_data).decode()

    try:
        if provider == "openai":
            async with aiohttp.ClientSession() as session:
                async with session.post(
                    "https://api.openai.com/v1/chat/completions",
                    headers={
                        "Authorization": f"Bearer {api_key}",
                        "Content-Type": "application/json",
                    },
                    json={
                        "model": model,
                        "messages": [
                            {
                                "role": "user",
                                "content": [
                                    {"type": "text", "text": prompt},
                                    {
                                        "type": "image_url",
                                        "image_url": {"url": f"data:image/jpeg;base64,{image_b64}"},
                                    },
                                ],
                            }
                        ],
                        "temperature": 0.2,
                    },
                ) as resp:
                    if resp.status != 200:
                        return {
                            "observation": "",
                            "think": "",
                            "action": event.get("type", ""),
                            "expectation": "",
                        }
                    data = await resp.json()
                    text = data.get("choices", [{}])[0].get("message", {}).get("content", "")
        else:
            async with aiohttp.ClientSession() as session:
                async with session.post(
                    "https://api.anthropic.com/v1/messages",
                    headers={
                        "x-api-key": api_key,
                        "anthropic-version": "2023-06-01",
                        "content-type": "application/json",
                    },
                    json={
                        "model": model,
                        "max_tokens": 1200,
                        "messages": [
                            {
                                "role": "user",
                                "content": [
                                    {"type": "text", "text": prompt},
                                    {
                                        "type": "image",
                                        "source": {
                                            "type": "base64",
                                            "media_type": "image/jpeg",
                                            "data": image_b64,
                                        },
                                    },
                                ],
                            }
                        ],
                    },
                ) as resp:
                    if resp.status != 200:
                        return {
                            "observation": "",
                            "think": "",
                            "action": event.get("type", ""),
                            "expectation": "",
                        }
                    data = await resp.json()
                    text = data.get("content", [{}])[0].get("text", "")

        # Parse JSON response
        import re

        json_match = re.search(r"\{[\s\S]*\}", text)
        if json_match:
            parsed = json.loads(json_match.group())
            return {
                "observation": parsed.get("Observation", parsed.get("observation", "")),
                "think": parsed.get("Think", parsed.get("think", "")),
                "action": parsed.get("Action", parsed.get("action", "")),
                "expectation": parsed.get("Expectation", parsed.get("expectation", "")),
            }
    except Exception:
        pass

    return {"observation": "", "think": "", "action": event.get("type", ""), "expectation": ""}
