# MCP Server Concurrent Session Management

This document describes the improvements made to the MCP Server to address concurrent session management and resource lifecycle issues.

## Problem Statement

The original MCP server implementation had several critical issues:

1. **Global Computer Instance**: Used a single `global_computer` variable shared across all clients
2. **No Resource Isolation**: Multiple clients would interfere with each other
3. **Sequential Task Processing**: Multi-task operations were always sequential
4. **No Graceful Shutdown**: Server couldn't properly cleanup resources on shutdown
5. **Hidden Event Loop**: `server.run()` hid the event loop, preventing proper lifecycle management

## Solution Architecture

### 1. Session Manager (`session_manager.py`)

The `SessionManager` class provides:

- **Per-session computer instances**: Each client gets isolated computer resources
- **Computer instance pooling**: Efficient reuse of computer instances with lifecycle management
- **Task registration**: Track active tasks per session for graceful cleanup
- **Automatic cleanup**: Background task cleans up idle sessions
- **Resource limits**: Configurable maximum concurrent sessions

#### Key Components:

```python
class SessionManager:
    def __init__(self, max_concurrent_sessions: int = 10):
        self._sessions: Dict[str, SessionInfo] = {}
        self._computer_pool = ComputerPool()
        # ... lifecycle management
```

#### Session Lifecycle:

1. **Creation**: New session created when client first connects
2. **Task Registration**: Each task is registered with the session
3. **Activity Tracking**: Last activity time updated on each operation
4. **Cleanup**: Sessions cleaned up when idle or on shutdown

### 2. Computer Pool (`ComputerPool`)

Manages computer instances efficiently:

- **Pool Size Limits**: Maximum number of concurrent computer instances
- **Instance Reuse**: Available instances reused across sessions
- **Lifecycle Management**: Proper startup/shutdown of computer instances
- **Resource Cleanup**: All instances properly closed on shutdown

### 3. Enhanced Server Tools

All server tools now support:

- **Session ID Parameter**: Optional `session_id` for multi-client support
- **Resource Isolation**: Each session gets its own computer instance
- **Task Tracking**: Proper registration/unregistration of tasks
- **Error Handling**: Graceful error handling with session cleanup

#### Updated Tool Signatures:

```python
async def screenshot_cua(ctx: Context, session_id: Optional[str] = None) -> Any:
async def run_cua_task(ctx: Context, task: str, session_id: Optional[str] = None) -> Any:
async def run_multi_cua_tasks(ctx: Context, tasks: List[str], session_id: Optional[str] = None, concurrent: bool = False) -> Any:
```

### 4. Concurrent Task Execution

The `run_multi_cua_tasks` tool now supports:

- **Sequential Mode** (default): Tasks run one after another
- **Concurrent Mode**: Tasks run in parallel using `asyncio.gather()`
- **Progress Tracking**: Proper progress reporting for both modes
- **Error Handling**: Individual task failures don't stop other tasks

### 5. Graceful Shutdown

The server now provides:

- **Signal Handlers**: Proper handling of SIGINT and SIGTERM
- **Session Cleanup**: All active sessions properly cleaned up
- **Resource Release**: Computer instances returned to pool and closed
- **Async Lifecycle**: Event loop properly exposed for cleanup

## Usage Examples

### Basic Usage (Backward Compatible)

```python
# These calls work exactly as before
await screenshot_cua(ctx)
await run_cua_task(ctx, "Open browser")
await run_multi_cua_tasks(ctx, ["Task 1", "Task 2"])
```

### Multi-Client Usage

```python
# Client 1
session_id_1 = "client-1-session"
await screenshot_cua(ctx, session_id_1)
await run_cua_task(ctx, "Open browser", session_id_1)

# Client 2 (completely isolated)
session_id_2 = "client-2-session"
await screenshot_cua(ctx, session_id_2)
await run_cua_task(ctx, "Open editor", session_id_2)
```

### Concurrent Task Execution

```python
# Run tasks concurrently instead of sequentially
tasks = ["Open browser", "Open editor", "Open terminal"]
results = await run_multi_cua_tasks(ctx, tasks, concurrent=True)
```

### Session Management

```python
# Get session statistics
stats = await get_session_stats(ctx)
print(f"Active sessions: {stats['total_sessions']}")

# Cleanup specific session
await cleanup_session(ctx, "session-to-cleanup")
```

## Configuration

### Environment Variables

- `CUA_MODEL_NAME`: Model to use (default: `anthropic/claude-3-5-sonnet-20241022`)
- `CUA_MAX_IMAGES`: Maximum images to keep (default: `3`)

### Session Manager Configuration

```python
# In session_manager.py
class SessionManager:
    def __init__(self, max_concurrent_sessions: int = 10):
        # Configurable maximum concurrent sessions

class ComputerPool:
    def __init__(self, max_size: int = 5, idle_timeout: float = 300.0):
        # Configurable pool size and idle timeout
```

## Performance Improvements

### Before (Issues):

- ❌ Single global computer instance
- ❌ Client interference and resource conflicts
- ❌ Sequential task processing only
- ❌ No graceful shutdown
- ❌ 30s timeout issues with long-running tasks

### After (Benefits):

- ✅ Per-session computer instances with proper isolation
- ✅ Computer instance pooling for efficient resource usage
- ✅ Concurrent task execution support
- ✅ Graceful shutdown with proper cleanup
- ✅ Streaming updates prevent timeout issues
- ✅ Configurable resource limits
- ✅ Automatic session cleanup

## Testing

Comprehensive test coverage includes:

- Session creation and reuse
- Concurrent session isolation
- Task registration and cleanup
- Error handling with session management
- Concurrent vs sequential task execution
- Session statistics and cleanup

Run tests with:

```bash
pytest tests/test_mcp_server_session_management.py -v
```

## Migration Guide

### For Existing Clients

No changes required! The new implementation is fully backward compatible:

```python
# This still works exactly as before
await run_cua_task(ctx, "My task")
```

### For New Multi-Client Applications

Use session IDs for proper isolation:

```python
# Create a unique session ID for each client
session_id = str(uuid.uuid4())
await run_cua_task(ctx, "My task", session_id)
```

### For Concurrent Task Execution

Enable concurrent mode for better performance:

```python
tasks = ["Task 1", "Task 2", "Task 3"]
results = await run_multi_cua_tasks(ctx, tasks, concurrent=True)
```

## Monitoring and Debugging

### Session Statistics

```python
stats = await get_session_stats(ctx)
print(f"Total sessions: {stats['total_sessions']}")
print(f"Max concurrent: {stats['max_concurrent']}")
for session_id, session_info in stats['sessions'].items():
    print(f"Session {session_id}: {session_info['active_tasks']} active tasks")
```

### Logging

The server provides detailed logging for:

- Session creation and cleanup
- Task registration and completion
- Resource pool usage
- Error conditions and recovery

### Graceful Shutdown

The server properly handles shutdown signals:

```bash
# Send SIGTERM for graceful shutdown
kill -TERM <server_pid>

# Or use Ctrl+C (SIGINT)
```

## Future Enhancements

Potential future improvements:

1. **Session Persistence**: Save/restore session state across restarts
2. **Load Balancing**: Distribute sessions across multiple server instances
3. **Resource Monitoring**: Real-time monitoring of resource usage
4. **Auto-scaling**: Dynamic adjustment of pool size based on demand
5. **Session Timeouts**: Configurable timeouts for different session types
