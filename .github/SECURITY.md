# Security Policy

## Reporting a Vulnerability

We take security vulnerabilities in Cua seriously. If you discover a security issue, please follow the responsible disclosure process below.

**Do not report security vulnerabilities through public GitHub Issues.**

### How to Report

Please report vulnerabilities using one of the following methods:

1. **GitHub Security Advisories (preferred):** Use the [GitHub private vulnerability reporting](https://github.com/trycua/cua/security/advisories/new) feature to submit a report confidentially.

2. **Email:** Send details to **security@cua.ai** with the subject line `[SECURITY] <brief description>`.

### What to Include

When reporting, please provide as much of the following as possible:

- Type of vulnerability (e.g., command injection, privilege escalation, credential leak)
- Component(s) affected (e.g., `computer-server`, `cua-cli`, `lume`)
- Steps to reproduce the issue
- Potential impact and severity
- Any suggested mitigations

### Response Timeline

| Stage | Target |
|-------|--------|
| Initial acknowledgment | ≤ 7 days |
| Triage and severity assessment | ≤ 14 days |
| Fix or mitigation plan communicated | ≤ 30 days |
| Public disclosure (coordinated) | After fix is released |

We aim to acknowledge all reports within **7 days** and provide a full response within **14 days**.

## Supported Versions

We actively maintain security fixes for the following:

| Component | Supported |
|-----------|-----------|
| Latest release of each package | ✅ |
| Prior minor releases | Case-by-case basis |

## Scope

This policy covers the following repositories and packages:

- `trycua/cua` — all packages (`cua-agent`, `cua-computer`, `cua-computer-server`, `cua-bench`, `cua-cli`, `lume`)

The following are **out of scope**:

- Vulnerabilities in third-party dependencies (please report upstream; we will track and patch via Dependabot)
- Issues in forks or unofficial distributions
- Social engineering or phishing attacks

## Disclosure Policy

We follow [coordinated vulnerability disclosure](https://vuls.cert.org/confluence/display/Wiki/Coordinated+Vulnerability+Disclosure+Guidance). We will:

1. Confirm receipt of your report
2. Assess impact and assign a CVE if warranted
3. Develop and test a fix
4. Release the fix and credit you in the release notes (unless you prefer anonymity)
5. Publicly disclose after users have had reasonable time to update

## Bug Bounty

We do not currently operate a paid bug bounty program. We recognize contributors in release notes and the project README.

---

Thank you for helping keep Cua and its users safe.
