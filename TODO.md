# CUA Snapshot Manager Integration TODO

## Current Status: Integration-First MVP Implementation
**Phase**: Core integration complete, implementing team-requested MVP features  
**Target**: Deliver production-ready snapshot management for CUA Agent SDK

## Completed Tasks
- [x] Draft PR created in CUA repo
- [x] ARCHITECTURE.md with concise technical overview
- [x] DockerSnapshotProvider wired to CUA callbacks
- [x] Integration tests with real CUA agent verified
- [x] CI compliance validated (Black, Ruff, pytest)

## Remaining MVP Requirements

### Named Volume Support
- [ ] Tar-based backup/restore for named volumes
- [ ] Bind mount detection with user warnings
- [ ] Volume type identification and handling strategy

### Enhanced Restore Modes
- [ ] Replace mode: stop/remove original, create replacement
- [ ] Minimal container healthcheck after restore
- [ ] Error handling for restore failures

### Deterministic Retention Policy
- [ ] Age-first cleanup (max_age then max_count)
- [ ] Deterministic ordering for snapshot removal
- [ ] Focused unit test for retention logic

## Technical Debt
- [ ] MyPy type annotation cleanup (46 non-critical errors)
- [ ] CLI function return type annotations
- [ ] Test function type hints

## Next Steps
1. Implement named volume tar support
2. Add replace restore mode with healthcheck
3. Implement deterministic retention policy
4. Address type annotations for cleaner codebase
5. Final CI validation before team review