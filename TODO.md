# CUA Snapshot Manager Integration TODO

## Current Status: Core MVP Features Complete
**Phase**: Named volume support completed, finalizing remaining MVP features  
**Target**: Production-ready snapshot management for CUA Agent SDK

## Completed Tasks ✅
- [x] Draft PR created in CUA repo
- [x] ARCHITECTURE.md with concise technical overview
- [x] DockerSnapshotProvider wired to CUA callbacks
- [x] Integration tests with real CUA agent verified
- [x] CI compliance validated (Black, Ruff, pytest)
- [x] **Named Volume Support** - Tar-based backup/restore for named volumes
- [x] **Bind Mount Detection** - User warnings for non-portable bind mounts
- [x] **Volume Type Identification** - Full volume analysis and handling
- [x] **Critical MyPy Fixes** - All blocking type errors resolved

## Remaining MVP Requirements

### Enhanced Restore Modes
- [ ] Replace mode: stop/remove original, create replacement
- [ ] Minimal container healthcheck after restore
- [ ] Error handling for restore failures

### Deterministic Retention Policy
- [ ] Age-first cleanup (max_age then max_count)
- [ ] Deterministic ordering for snapshot removal
- [ ] Focused unit test for retention logic

### Final Polish
- [ ] CI validation in pipeline
- [ ] Documentation review and finalization

## Technical Debt (Non-blocking)
- [ ] MyPy type annotation cleanup (1 false positive remaining)
- [ ] CLI function return type annotations
- [ ] Test function type hints

## Test Coverage
- **19/19 tests passing** including comprehensive volume support tests
- Volume detection, backup, storage, restore, and error handling all verified
- CUA integration examples working end-to-end

## Next Steps
1. Add replace restore mode with healthcheck
2. Implement deterministic retention policy  
3. Final CI validation before team review