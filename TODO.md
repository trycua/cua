# 🎯 CUA Snapshot Manager Integration TODO

## Current Status: DRAFT PR CREATED ✅
**Draft PR**: Ready for team review and CI validation  
**Goal**: Complete integration-first MVP as requested by team

---

## 📋 IMMEDIATE PRIORITY: Ensure CI Passes

### ✅ COMPLETED
- [x] Draft PR created in CUA repo for team review
- [x] ARCHITECTURE.md (≤400 words) with ASCII diagram
- [x] Basic snapshot manager integration in CUA structure
- [x] DockerSnapshotProvider implementation 
- [x] CUA-style module documentation

### 🔧 CI VALIDATION (URGENT - DO FIRST)
**Team requires CI to be green before evaluating broader design**

#### Step 1: Local CI Checks
```bash
# Run CUA's formatting and validation tools
pdm run black libs/python/snapshot-manager/
pdm run ruff check --fix libs/python/snapshot-manager/
pdm run mypy libs/python/snapshot-manager/
```

#### Step 2: Fix Any CI Issues
- [ ] **Black formatting** - Fix code style issues
- [ ] **Ruff linting** - Fix import/style violations  
- [ ] **MyPy typing** - Add missing type annotations
- [ ] **Test compatibility** - Ensure tests run in CUA environment

#### Step 3: Integration with CUA Root
- [ ] **Add to root pyproject.toml** - Include snapshot-manager in dev dependencies
- [ ] **Update test paths** - Ensure pytest finds our tests
- [ ] **Verify imports** - Check CUA SDK dependencies work

---

## 🚀 INTEGRATION-FIRST MVP REQUIREMENTS
**Complete ONLY after CI passes**

### Priority 1: DockerSnapshotProvider → CUA Callbacks Integration
- [ ] **Wire to callbacks** - Explicit DockerSnapshotProvider + SnapshotCallback integration
- [ ] **Integration test** - Real CUA agent with automatic snapshots working
- [ ] **Container resolution** - Fix container ID resolution from CUA context

### Priority 2: Named Volume Support  
- [ ] **Tar backup/restore** - Support named-volume snapshot/restore using tar
- [ ] **Bind mount warnings** - Warn on bind mounts (can't backup host files)
- [ ] **Volume detection** - Identify and handle different mount types

### Priority 3: Restore Modes
- [ ] **Replace mode** - Stop/remove original, create replacement with same name  
- [ ] **New container mode** - Create new container (already implemented)
- [ ] **Minimal healthcheck** - Basic validation that restored container works

### Priority 4: Deterministic Retention
- [ ] **Age-first policy** - Delete by max_age THEN max_count (not OR)
- [ ] **Deterministic order** - Ensure consistent cleanup behavior
- [ ] **Small focused test** - Test specifically this retention logic

---

## 📊 VALIDATION CHECKLIST

### Local Testing
- [ ] `pdm run black .` - Code formatting passes
- [ ] `pdm run ruff check --fix .` - Linting passes  
- [ ] `pdm run mypy .` - Type checking passes
- [ ] `pytest libs/python/snapshot-manager/tests/` - Tests pass
- [ ] Manual CLI test - Basic snapshot operations work

### Integration Testing  
- [ ] CUA agent + snapshots - End-to-end workflow works
- [ ] Docker operations - Create/restore/delete snapshots  
- [ ] Callback triggers - Automatic snapshots on agent lifecycle
- [ ] Error handling - Graceful failures don't break agent

### CI Requirements
- [ ] GitHub Actions CI - All checks pass
- [ ] No import errors - CUA SDK dependencies resolve
- [ ] Test discovery - pytest finds and runs our tests
- [ ] Code quality - Meets CUA standards (black, ruff, mypy)

---

## 🎯 SUCCESS CRITERIA
**Team will evaluate broader design AFTER:**
1. ✅ CI is green (all checks pass)
2. ✅ Integration-first MVP features complete
3. ✅ Real CUA agent workflow demonstrable

## ⚠️ RISK MITIGATION
**Do NOT implement additional features until CI passes**
- Focus on fixing any linting/typing/formatting issues first
- Get the foundation solid before adding complexity
- Team wants to see green CI before architecture evaluation
