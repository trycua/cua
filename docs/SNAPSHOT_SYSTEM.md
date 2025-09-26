# CUA Snapshot System

## Overview

The CUA Snapshot System enables creating, managing, and restoring filesystem snapshots of Docker containers during agent execution. This provides checkpoint/restore functionality for debugging, error recovery, and state management.

## Core Functionality

### Snapshot Operations

- **Create**: Capture current container state as a Docker image
- **Restore**: Roll back container to a previous snapshot state  
- **List**: View all available snapshots with metadata
- **Delete**: Remove snapshots to free storage space

### Scheduling Options

- `manual`: Create snapshots only when explicitly requested
- `every_action`: Snapshot after each computer action (debugging)
- `run_start`: Snapshot at the beginning of each agent run
- `run_end`: Snapshot at the end of each agent run  
- `run_boundaries`: Snapshot at both start and end (recommended)

### Retention Management

- **Count-based**: Keep only the N most recent snapshots
- **Age-based**: Delete snapshots older than specified days
- **Automatic cleanup**: Configurable background cleanup

## Basic Usage

```python
from computer import Computer
from agent import ComputerAgent
from libs.python.agent.agent.callbacks.snapshot_manager import SnapshotManagerCallback

# Setup computer with Docker provider
computer = Computer(
    os_type="linux",
    provider_type="docker", 
    image="trycua/cua-ubuntu:latest",
    name='my-container'
)

# Configure snapshot callback
snapshot_callback = SnapshotManagerCallback(
    computer=computer,
    snapshot_interval="run_boundaries",  # When to create snapshots
    max_snapshots=10,                    # Keep latest 10 snapshots
    retention_days=7,                    # Delete snapshots older than 7 days
    auto_cleanup=True                    # Enable automatic cleanup
)

# Create agent with snapshot support
agent = ComputerAgent(
    model="claude-3-5-sonnet-20241022",
    callbacks=[snapshot_callback]
)
```

## Manual Operations

```python
# Create manual snapshot
result = await snapshot_callback.create_manual_snapshot("before-risky-operation")

# List all snapshots  
snapshots = await snapshot_callback.list_snapshots()

# Restore to specific snapshot
restore_result = await snapshot_callback.restore_snapshot(snapshot_id)

# Delete snapshot
delete_result = await snapshot_callback.delete_snapshot(snapshot_id)
```

## Configuration

| Parameter | Default | Description |
|-----------|---------|-------------|
| `snapshot_interval` | "manual" | When to create snapshots automatically |
| `max_snapshots` | 10 | Maximum snapshots to retain |
| `retention_days` | 7 | Delete snapshots older than N days |
| `auto_cleanup` | True | Enable automatic cleanup |
| `metadata_dir` | "/tmp/cua_snapshots" | Metadata storage location |

## Implementation Details

### Docker Integration

Snapshots are implemented using Docker's native `docker commit` functionality:

- Container state is captured as a Docker image
- Metadata stored as image labels (prefixed with `cua.snapshot.`)
- Container configuration (memory, CPU, ports) preserved during restore
- Unique snapshot IDs generated with timestamps

### Provider Interface

VM providers must implement these methods:

```python
async def create_snapshot(name: str, snapshot_name: str = None, metadata: dict = None) -> dict
async def restore_snapshot(name: str, snapshot_id: str) -> dict  
async def list_snapshots(name: str) -> List[dict]
async def delete_snapshot(snapshot_id: str) -> dict
```

### Error Handling

All operations return status information:

```python
# Success response
{
    "id": "uuid-v4-string",
    "status": "created", 
    "timestamp": "2024-01-01T12:00:00",
    "size": "150MB"
}

# Error response  
{
    "status": "error",
    "error": "Detailed error message"
}
```

## Best Practices

1. Use `run_boundaries` interval for most cases
2. Set reasonable retention limits to manage storage
3. Include descriptive metadata for easier identification
4. Test restore procedures regularly
5. Monitor disk space usage
6. Handle operation errors gracefully
