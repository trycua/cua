# Snapshot Manager Architecture

## Overview
The CUA Snapshot Manager provides state capture and restoration for Docker containers during AI agent execution, enabling rollback, debugging, and reliable state recovery.

## System Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        CUA Agent SDK                           │
├─────────────────────────────────────────────────────────────────┤
│  ComputerAgent  │  Callbacks  │  Computer  │  Run Lifecycle    │
└─────────────────┬───────────────────────────────────────────────┘
                  │
                  ▼
        ┌─────────────────────┐
        │  SnapshotCallback   │ ◄─── Automatic Triggers
        │  (CUA Integration)  │      (run_start, run_end, etc.)
        └─────────┬───────────┘
                  │
                  ▼
        ┌─────────────────────┐
        │   SnapshotManager   │ ◄─── Manual API Calls
        │  (Central Control)  │      (CLI, Programmatic)
        └─────────┬───────────┘
                  │
         ┌────────┴────────┐
         ▼                 ▼
┌─────────────────┐ ┌─────────────────┐
│ SnapshotProvider│ │ SnapshotStorage │
│   (Abstract)    │ │   (Abstract)    │
└─────────┬───────┘ └─────────┬───────┘
          │                   │
          ▼                   ▼
┌─────────────────┐ ┌─────────────────┐
│ DockerSnapshot  │ │ FileSystemSnap  │
│    Provider     │ │   shotStorage   │
│                 │ │                 │
│ • docker commit │ │ • JSON metadata │
│ • container ops │ │ • indexing      │
│ • validation    │ │ • statistics    │
└─────────┬───────┘ └─────────┬───────┘
          │                   │
          ▼                   ▼
┌─────────────────┐ ┌─────────────────┐
│   Docker API    │ │   File System   │
│                 │ │                 │
│ • Images        │ │ • ./snapshots/  │
│ • Containers    │ │   ├─metadata/   │
│ • Commit/Run    │ │   └─index.json  │
└─────────────────┘ └─────────────────┘
```

## What's Captured
**Container State:**
- Complete filesystem state via `docker commit` (creates new Docker image)
- Container configuration (environment variables, working directory, user, command)
- Mount point metadata (named volumes, bind mounts)
- Network settings and port mappings
- Labels and original image reference

**Metadata:**
- Timestamp and trigger context (manual, run_start, run_end, before/after actions)
- Human-readable descriptions and agent run IDs
- Storage statistics and restoration tracking
- Container relationship mapping

## What's NOT Captured
**Excluded by Design:**
- Live running processes and their memory state
- Temporary filesystems (`/tmp`, `/proc`, `/sys`, `/dev`)
- Real-time network connections and sockets
- Named volume *contents* (currently - planned via tar backup)
- Host-mounted bind mount contents (warned, not captured)

**Technical Limitations:**
- Containers must be in valid state (running/paused/exited/created)
- Docker daemon must be accessible and responsive
- No cross-host container migration support

## Restore Behavior
**New Container Mode (Default):**
- Creates fresh container from snapshot image
- Generates unique name with timestamp suffix
- Preserves original configuration and mounts (if available)
- Original container remains untouched

**Replace Mode (Planned):**
- Stops and removes target container
- Creates replacement with same name and configuration
- Includes basic health check validation

**Volume Handling:**
- Named volumes: Preserved by reference (data intact if volume exists)
- Bind mounts: Configuration preserved, warns if host path unavailable
- Anonymous volumes: Lost (Docker limitation)

## Limits & Policies
**Storage Limits:**
- Per-container snapshot count (default: 10)
- Global snapshot count (default: 100)  
- Total storage size (default: 10GB)

**Retention Policy (Deterministic):**
- Primary: Age-based cleanup (default: 7 days)
- Secondary: Count-based limit enforcement (oldest first)
- Execution order: max_age THEN max_count

**Operation Safety:**
- Concurrent operation locks per container
- Graceful degradation (snapshot failures don't break agent execution)
- Atomic metadata updates (index updated only after successful snapshot)
- Orphaned metadata cleanup on storage corruption

## Integration Points
**CUA Agent SDK:**
- Pluggable callback handler (enable/disable without code changes)
- Configurable trigger points (run lifecycle, action boundaries)
- Non-blocking operation design (async throughout)

**Storage Backend:**
- JSON-based metadata with efficient indexing
- Pluggable storage interface (filesystem, future: cloud/database)
- Human-readable format for debugging and inspection

**Docker Provider:**
- Uses Docker's native `docker commit` for efficiency
- Leverages Docker image layer sharing for space optimization
- Direct integration with Docker API (no CLI shelling)
