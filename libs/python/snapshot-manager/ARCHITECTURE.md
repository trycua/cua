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
│ • volume backup │ │ • volume tar    │
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
│ • Volumes       │ │   ├─volumes/    │
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

**Named Volume Data:**
- Full volume contents backed up as tar archives
- Volume analysis with type identification (named vs bind mounts)
- Automatic volume restoration with data integrity verification
- Bind mount detection with portability warnings

**Metadata:**
- Timestamp and trigger context (manual, run_start, run_end, before/after actions)
- Human-readable descriptions and agent run IDs
- Storage statistics and restoration tracking
- Container relationship mapping
- Volume backup manifest and storage locations

## What's NOT Captured
**Excluded by Design:**
- Live running processes and their memory state
- Temporary filesystems (`/tmp`, `/proc`, `/sys`, `/dev`)
- Real-time network connections and sockets
- Host-mounted bind mount contents (warned about, not portable across systems)
- Anonymous volumes (Docker limitation, data lost on container removal)

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
- Named volumes: Full data backup as tar archives, automatic restoration with integrity verification
- Bind mounts: Configuration preserved, warnings displayed about portability concerns
- Anonymous volumes: Lost (Docker limitation, not backed up)

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

## Volume Backup & Restore Logic

**Volume Detection:**
1. Analyze container mount configuration via Docker API
2. Categorize mounts by type: named volumes vs bind mounts
3. Generate volume analysis report with portability warnings

**Backup Process:**
1. For each named volume attached to container:
   - Create temporary alpine container with volume mounted read-only
   - Extract complete volume data using `docker get_archive` (tar format)
   - Store tar archives in `./snapshots/volumes/{snapshot_id}_{volume_name}.tar`
2. Log warnings for bind mounts (not included in backup)
3. Record volume manifest in snapshot metadata

**Restore Process:**
1. Create or verify target volumes exist in Docker
2. For each backed-up volume:
   - Create temporary alpine container with volume mounted read-write
   - Clear existing volume contents (ensuring clean restore)
   - Extract tar data using `docker put_archive`
   - Verify extraction success
3. Attach restored volumes to new container with original mount paths

**Storage Structure:**
```
./snapshots/
├── metadata/{snapshot_id}.json     # Container + volume metadata
├── volumes/{snapshot_id}_{vol}.tar  # Volume data archives
└── index.json                      # Global snapshot index
```
