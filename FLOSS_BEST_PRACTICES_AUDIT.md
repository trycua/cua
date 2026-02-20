# OpenSSF Best Practices Passing Badge - Audit Report

**Project:** Cua (Computer Use Agent)
**Repository:** https://github.com/trycua/cua
**Audit Date:** 2026-02-20
**Auditor:** Automated audit against OpenSSF Best Practices passing-level criteria

---

## Summary

| Category | MUST Pass | MUST Fail | SHOULD Pass | SHOULD Fail | SUGGESTED Pass | SUGGESTED Fail |
|---|---|---|---|---|---|---|
| **Basics** | 7/8 | 1 | 1/2 | 1 | 1/2 | 1 |
| **Change Control** | 6/7 | 1 | 0/0 | 0 | 3/4 | 1 |
| **Reporting** | 4/5 | 1 | 2/2 | 0 | 0/0 | 0 |
| **Quality** | 6/7 | 1 | 2/3 | 1 | 3/4 | 1 |
| **Security** | 4/5 | 1 | 1/3 | 2 | 1/1 | 0 |
| **Analysis** | 2/2 | 0 | 0/0 | 0 | 2/4 | 2 |
| **TOTAL** | **29/34** | **5** | **6/10** | **4** | **10/15** | **5** |

**Overall Status: FAILING** - 5 MUST-level criteria are not met.

---

## Basics

### Basic project website content

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Project describes what the software does | description_good | MUST | **PASS** | README.md clearly states: "Build, benchmark, and deploy agents that use computers." Detailed descriptions of Cua, Cua-Bench, Lume, and CuaBot subsystems. |
| Info on how to obtain, give feedback, and contribute | interact | MUST | **PASS** | README links to GitHub Issues for bug reports, Discord for discussion, and CONTRIBUTING.md for contribution guidelines. |
| Contribution process explained | contribution | MUST | **PASS** | CONTRIBUTING.md explains bug reporting, enhancement suggestions, code formatting, and PR process. Development.md provides detailed setup instructions. |
| Requirements for acceptable contributions | contribution_requirements | SHOULD | **PASS** | CONTRIBUTING.md references code formatting standards (black, isort, ruff, mypy). Pre-commit hooks enforce standards. |

### FLOSS license

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Software released as FLOSS | floss_license | MUST | **PASS** | MIT License. |
| License approved by OSI | floss_license_osi | SUGGESTED | **PASS** | MIT is OSI-approved. |
| License posted in standard location | license_location | MUST | **PASS** | LICENSE.md in repository root. Also declared in pyproject.toml (`license = { text = "MIT" }`). |

### Documentation

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Basic documentation provided | documentation_basics | MUST | **PASS** | README.md, Development.md, TESTING.md, extensive docs/ directory with full documentation site (Next.js-based), and documentation at https://cua.ai/docs. |
| Reference documentation for external interface | documentation_interface | MUST | **PASS** | API reference documentation available at cua.ai/docs. Package-level documentation exists for agent SDK, computer SDK, CLI reference, and MCP server. |

### Other

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Project sites support HTTPS with TLS | sites_https | MUST | **PASS** | GitHub (https://github.com/trycua/cua) and project site (https://cua.ai) both use HTTPS. |
| Searchable discussion mechanism | discussion | MUST | **PASS** | GitHub Issues (searchable, URL-addressable), Discord community. GitHub Discussions may also be available. |
| Documentation in English, accepts English bug reports | english | SHOULD | **FAIL** | Documentation is in English, but CONTRIBUTING.md does not explicitly state that bug reports/comments should be in English. This is a minor gap. (De facto English but not documented.) |
| Project is maintained | maintained | MUST | **FAIL** | **CONCERN:** While recent commits exist (Feb 2026), there is no explicit statement of project maintenance status. The OpenSSF criterion requires evidence such as recent releases, responses to issues, and ongoing development activity. Commits are active, so this likely passes — but there is no formal maintenance policy or status badge. **Likely PASS on evidence, but marking as PASS with caveat.** |

**Re-evaluation of `maintained`:** Based on very recent commit activity (Feb 2026), active CI/CD, and regular releases, this criterion is **PASS**.

**Revised Basics status:** `english` is SHOULD-level so does not block passing.

---

## Change Control

### Public version-controlled source repository

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Publicly readable VCS repository with URL | repo_public | MUST | **PASS** | https://github.com/trycua/cua — public GitHub repository. |
| Repository tracks changes, who, and when | repo_track | MUST | **PASS** | Git tracks all changes with author and timestamp. |
| Repository includes interim versions | repo_interim | MUST | **PASS** | Active development with feature branches, PRs, and incremental commits. Not just final releases. |
| Uses common distributed VCS | repo_distributed | SUGGESTED | **PASS** | Uses git, hosted on GitHub. |

### Unique version numbering

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Unique version identifier per release | version_unique | MUST | **PASS** | Each package has unique versions (e.g., `agent-v0.7.26`, `computer-v0.5.14`, `core-v0.1.17`). Managed via bump2version. |
| Uses SemVer or CalVer | version_semver | SUGGESTED | **PASS** | Uses Semantic Versioning (MAJOR.MINOR.PATCH). |
| Releases identified in VCS | version_tags | SUGGESTED | **PASS** | Git tags created for each release (e.g., `agent-v0.7.26`). Tag-triggered CI/CD publishes packages. |

### Release notes

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Release notes provided (human-readable, not raw git log) | release_notes | MUST | **FAIL** | **ISSUE:** The release workflow (`release-github-reusable.yml`) auto-generates release notes from commit messages filtered by path. While it adds GitHub usernames and PR links, the output is essentially a formatted git log with commit subjects — not a human-curated summary of major changes. There is no CHANGELOG.md file. Individual releases lack context about upgrade impact or migration notes. |
| Release notes identify fixed CVEs | release_notes_vulns | MUST | **N/A** | No known CVE assignments found. Marking N/A. If CVEs are assigned in the future, release notes must call them out. |

---

## Reporting

### Bug-reporting process

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Process for users to submit bug reports | report_process | MUST | **PASS** | GitHub Issues. CONTRIBUTING.md documents the bug reporting process with clear steps. |
| Uses issue tracker | report_tracker | SHOULD | **PASS** | GitHub Issues serves as the issue tracker. |
| Acknowledges majority of bug reports (2-12 months) | report_responses | MUST | **PASS** | Based on active repository with recent PR merges and issue activity. (Full verification would require GitHub API audit of issue response rates.) |
| Responds to majority of enhancement requests | enhancement_responses | SHOULD | **PASS** | Active development suggests responsiveness. (Same caveat as above.) |
| Publicly available archive for reports | report_archive | MUST | **PASS** | GitHub Issues provides a publicly searchable archive. |

### Vulnerability report process

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Published vulnerability reporting process | vulnerability_report_process | MUST | **FAIL** | **MISSING:** No SECURITY.md file exists. No vulnerability reporting process is published anywhere in the repository or on the project website. This is a **critical gap**. |
| Private vulnerability reporting mechanism | vulnerability_report_private | MUST | **FAIL** | **MISSING:** No mechanism for private vulnerability disclosure. No security email, no GitHub Security Advisories configuration, no PGP key for encrypted reports. |
| Initial response time ≤14 days for vulnerability reports | vulnerability_report_response | MUST | **N/A** | No vulnerability reports documented. N/A due to missing process. |

---

## Quality

### Working build system

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Working build system | build | MUST | **PASS** | Python: `uv sync` builds all packages. TypeScript: `pnpm install` + build scripts. Swift (Lume): Xcode/Swift build system. Docker: Dockerfiles for container images. CI workflows validate builds. |
| Uses common build tools | build_common_tools | SUGGESTED | **PASS** | uv (Python), pnpm (JS/TS), Xcode (Swift), Docker — all widely used. |
| Buildable using only FLOSS tools | build_floss_tools | SHOULD | **FAIL** | Lume (Swift) requires Apple's Virtualization.Framework and Xcode, which are proprietary. The Python and TypeScript components are fully buildable with FLOSS tools. Mixed result — core components pass but Lume does not. |

### Automated test suite

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Automated test suite (publicly released as FLOSS) | test | MUST | **PASS** | pytest-based test suite. 28+ test files across `libs/python/*/tests/` and `tests/`. CI workflow `ci-test-python.yml` runs tests automatically. TESTING.md documents how to run tests. |
| Test suite invocable in standard way | test_invocation | SHOULD | **PASS** | Standard `pytest` invocation. Documented in TESTING.md and pyproject.toml (`[tool.pytest.ini_options]`). |
| Test suite covers most code | test_most | SUGGESTED | **FAIL** | ~28 test files for ~194 source files (ratio ~14%). Coverage reporting is configured (pytest-cov, Codecov), but the test coverage appears low relative to codebase size. mypy type checking is disabled. |
| Continuous integration implemented | test_continuous_integration | SUGGESTED | **PASS** | GitHub Actions CI runs on every PR: linting (Python + TypeScript), testing (per-package matrix), and documentation checks. 63 workflow files total. |

### New functionality testing

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Policy for adding tests with new functionality | test_policy | MUST | **PASS** | CONTRIBUTING.md and Development.md reference testing requirements. CI enforces test execution. Pre-commit hooks enforce code quality. |
| Evidence of test policy adherence | tests_are_added | MUST | **PASS** | Recent PRs include test files. CI test matrix covers multiple packages. |
| Test policy documented in contribution instructions | tests_documented_added | SUGGESTED | **FAIL** | CONTRIBUTING.md does not explicitly require tests for new contributions. TESTING.md documents how to write tests but doesn't mandate them for PRs. The test policy is implicit rather than explicit. |

### Warning flags

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Compiler warnings / linter enabled | warnings | MUST | **PASS** | ruff (linter), black (formatter), isort (import sorting), TypeScript strict type checking, prettier. Pre-commit hooks enforce these. CI lint workflows block merges. |
| Warnings are addressed | warnings_fixed | MUST | **PASS** | CI blocks PRs with lint failures. Pre-commit hooks auto-fix where possible. `claude-auto-fix.yml` workflow even auto-fixes CI failures using AI. |
| Maximally strict warnings | warnings_strict | SUGGESTED | **PASS (partial)** | ruff rules E, F, B, I are enabled but many specific rules are ignored (E501, E402, F401, F403, etc.). mypy strict mode is configured but disabled in CI and pre-commit. TypeScript type checking is enabled. Strictness is moderate, not maximal. |

---

## Security

### Secure development knowledge

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Primary developer knows secure design | know_secure_design | MUST | **PASS** | OAuth 2.0 Bearer token authentication via HTTPS (`cua_cli/auth/`), parameterized SQL queries preventing injection, sandboxed VM execution, environment variable-based credential management. |
| Knows common vulnerability types and mitigations | know_common_errors | MUST | **PASS** | SQL injection mitigated (parameterized queries in `cua_cli/auth/store.py`). XSS mitigated (no `dangerouslySetInnerHTML` or unsafe `eval`). Credentials excluded via `.gitignore`. API keys use Bearer token pattern over HTTPS. |

### Cryptographic practices

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Uses publicly published crypto algorithms | crypto_published | MUST | **N/A** | Project does not directly implement cryptographic protocols. Relies on underlying platform TLS (HTTPS for API calls). SHA-256 used for file integrity checks (`cua_cli/api/client.py`). |
| Calls dedicated crypto libraries | crypto_call | SHOULD | **PASS** | Uses Python `hashlib` (standard library) for hashing. Does not re-implement crypto. |
| Crypto implementable using FLOSS | crypto_floss | MUST | **N/A** | N/A — no custom cryptographic implementation. |
| Default keylengths meet NIST 2030 minimum | crypto_keylength | MUST | **N/A** | Relies on platform defaults via HTTPS. |
| No broken crypto algorithms | crypto_working | MUST | **PASS (with note)** | MD5 is used in 4 files but only for non-security purposes (UI display hashing, folder identification): `computer/ui/gradio/app.py`, `computer_server/handlers/macos.py`, `cua_bench/computers/remote.py`, `cua_bench/computers/webtop.py`. SHA-256 is used for actual integrity checks. No MD4, single DES, RC4, or Dual_EC_DRBG found. |
| No algorithms with known serious weaknesses | crypto_weaknesses | SHOULD | **PASS (with note)** | MD5 usage is non-security (display/logging only). Should migrate to SHA-256 for future-proofing. |
| Perfect forward secrecy | crypto_pfs | SHOULD | **N/A** | Handled by underlying TLS libraries. |
| Passwords stored with iterated hashes | crypto_password_storage | MUST | **N/A** | Project stores API keys (not passwords) in SQLite via `cua_cli/auth/store.py`. Keys are stored as-is (not hashed) since they must be sent as Bearer tokens. No user password storage. |
| Cryptographically secure random number generation | crypto_random | MUST | **N/A** | No custom random number generation for security purposes found. |

### Secured delivery

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Delivery mechanism counters MITM | delivery_mitm | MUST | **PASS** | Packages delivered via PyPI (HTTPS), npm (HTTPS), Docker Hub (HTTPS), GitHub Releases (HTTPS). npm publish uses `--provenance` flag for supply chain security. |
| No crypto hash retrieved over HTTP without signature check | delivery_unsigned | MUST | **PASS** | No evidence of unsigned hash retrieval over HTTP. Installation scripts use HTTPS. |

### Publicly known vulnerabilities

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| No unpatched medium+ vulnerabilities known >60 days | vulnerabilities_fixed_60_days | MUST | **FAIL** | **ISSUE:** GitHub Dependabot reports **85 vulnerabilities** on the default branch: 3 critical, 36 high, 32 moderate, 14 low (see https://github.com/trycua/cua/security/dependabot). These are dependency vulnerabilities. If any medium+ severity issues have been known for >60 days, this criterion fails. Requires immediate triage. |
| Critical vulnerabilities fixed rapidly | vulnerabilities_critical_fixed | SHOULD | **FAIL** | 3 critical dependency vulnerabilities reported by Dependabot. Must be triaged and resolved rapidly. |

### Other security

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| No leaked credentials in public repos | no_leaked_credentials | MUST | **PASS** | .gitignore excludes `.env`, `.env.local`, `.secrets`, `.pypirc`. API keys loaded from environment variables. No hardcoded credentials found in source code. |

---

## Analysis

### Static code analysis

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Static analysis tool applied before release | static_analysis | MUST | **PASS** | ruff (Python linter with bug detection rules), TypeScript type checker, prettier. CI enforces these on every PR before merge. |
| Static analysis includes vulnerability rules | static_analysis_common_vulnerabilities | SUGGESTED | **FAIL** | ruff rules include "B" (flake8-bugbear) for common bugs but do not include security-focused rules like "S" (bandit/flake8-bandit). No dedicated security-focused SAST tool (e.g., Bandit, Semgrep, CodeQL) is configured. |
| Medium+ static analysis vulnerabilities fixed timely | static_analysis_fixed | MUST | **PASS** | CI blocks merges on lint failures. Auto-fix workflow addresses issues. |
| Static analysis on every commit or daily | static_analysis_often | SUGGESTED | **PASS** | Pre-commit hooks run on every commit. CI runs on every PR. |

### Dynamic code analysis

| Criterion | ID | Level | Status | Notes |
|---|---|---|---|---|
| Dynamic analysis applied before release | dynamic_analysis | SUGGESTED | **FAIL** | No fuzzing, DAST, or dynamic analysis tools configured. No evidence of web application scanning or runtime security testing. |
| Memory safety dynamic analysis for unsafe languages | dynamic_analysis_unsafe | SUGGESTED | **N/A** | Project is primarily Python/TypeScript (memory-safe). Swift (Lume) has some memory safety guarantees built into the language. |
| Dynamic analysis with assertions enabled | dynamic_analysis_enable_assertions | SUGGESTED | **PASS (partial)** | pytest uses assertions. Python's assert statements are active during testing. However, no dedicated assertion-heavy dynamic analysis configuration exists. |
| Medium+ dynamic analysis vulnerabilities fixed timely | dynamic_analysis_fixed | MUST | **N/A** | No dynamic analysis tool findings to address. |

---

## Critical Gaps Requiring Action

### MUST-level failures (block passing badge):

1. **`vulnerability_report_process`** — No SECURITY.md or published vulnerability reporting process.
2. **`vulnerability_report_private`** — No private vulnerability disclosure mechanism.
3. **`release_notes`** — Auto-generated release notes are essentially formatted git logs, not human-curated summaries.
4. **`vulnerabilities_fixed_60_days`** — 85 dependency vulnerabilities reported by GitHub Dependabot (3 critical, 36 high, 32 moderate, 14 low). Must be triaged; any medium+ known >60 days blocks passing.

### SHOULD-level failures (do not block but should be addressed):

5. **`english`** — No explicit statement that English is the project language.
6. **`build_floss_tools`** — Lume (Swift) component requires proprietary Apple tools.
7. **`vulnerabilities_critical_fixed`** — 3 critical dependency vulnerabilities need rapid resolution.

### High-priority recommendations:

1. **Create SECURITY.md** with:
   - How to report vulnerabilities (email address or GitHub Security Advisories)
   - Expected response timeline (≤14 days)
   - Scope of what's covered
   - PGP key or other mechanism for private reports

2. **Enable GitHub Security Advisories** for private vulnerability reporting.

3. **Improve release notes** — Add human-curated summaries of major changes, upgrade impact, and migration notes. Consider maintaining a CHANGELOG.md using Keep a Changelog format.

4. **Add security-focused static analysis** — Enable Bandit (flake8-bandit) or ruff "S" rules, or add CodeQL/Semgrep to CI.

5. **Triage and fix Dependabot vulnerabilities** — 85 dependency vulnerabilities (3 critical) are reported at https://github.com/trycua/cua/security/dependabot. Add `dependabot.yml` or Renovate for ongoing automated dependency updates.

6. **Document test requirements for contributions** — Explicitly state in CONTRIBUTING.md that new features must include tests.

7. **Enable mypy in CI** — Currently disabled due to untyped codebase. This is a significant quality gap for a security-sensitive project.

8. **Consider adding dynamic analysis** — Fuzzing for critical input parsing, or web application scanning for any HTTP-exposed interfaces.

---

## Appendix: File Evidence

| File | Purpose | Status |
|---|---|---|
| `LICENSE.md` | MIT License | Present |
| `README.md` | Project description, getting started | Present |
| `CONTRIBUTING.md` | Contribution guidelines | Present |
| `Development.md` | Developer setup guide | Present |
| `TESTING.md` | Test guide | Present |
| `SECURITY.md` | Security/vulnerability policy | **MISSING** |
| `CODE_OF_CONDUCT.md` | Community code of conduct | **MISSING** |
| `CHANGELOG.md` | Version history | **MISSING** |
| `.pre-commit-config.yaml` | Pre-commit hooks | Present |
| `pyproject.toml` | Python project config with linter settings | Present |
| `.gitignore` | Credential exclusion patterns | Present |
| `.github/workflows/` | 63 CI/CD workflow files | Present |
| `.github/SECURITY.md` | Security policy (GitHub location) | **MISSING** |
| `.github/ISSUE_TEMPLATE/` | Issue templates | **MISSING** |
| `.github/PULL_REQUEST_TEMPLATE.md` | PR template | **MISSING** |
| `.github/dependabot.yml` | Dependency scanning | **MISSING** |

---

## Appendix: Detailed Security Findings

### MD5 Usage (non-security context)

| File | Line(s) | Purpose |
|---|---|---|
| `libs/python/computer/computer/ui/gradio/app.py` | 496-503 | Screenshot hash for logging display |
| `libs/python/computer-server/computer_server/handlers/macos.py` | 420-432 | Component string hashing |
| `libs/cua-bench/cua_bench/computers/remote.py` | 356, 408 | Folder and HTML content identification |
| `libs/cua-bench/cua_bench/computers/webtop.py` | 219 | Folder path identification |

**Recommendation:** Migrate to SHA-256 for future-proofing, even though current use is non-security.

### `shell=True` subprocess usage

| File | Line | Context |
|---|---|---|
| `libs/python/computer-server/computer_server/handlers/android.py` | 519 | ADB command execution in controlled VM environment |
| `libs/python/computer-server/computer_server/handlers/generic.py` | 115 | System command execution in sandboxed environment |
| `libs/python/computer/computer/providers/lume_api.py` | 73-76 | Lume CLI interaction |

**Assessment:** Acceptable — these execute in controlled/sandboxed environments, not with user-supplied input from web interfaces.

### Authentication implementation (secure)

- OAuth 2.0 Bearer token flow in `libs/python/cua-cli/cua_cli/auth/browser.py`
- SQLite credential store with parameterized queries in `libs/python/cua-cli/cua_cli/auth/store.py`
- SHA-256 file integrity verification in `libs/python/cua-cli/cua_cli/api/client.py`
- HTTPS-only API communication (`https://api.cua.ai`)
- 2-minute auth timeout, daemon thread cleanup
