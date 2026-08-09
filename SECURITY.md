# Security Policy

## Report a Vulnerability

Do not open a public issue, RFC, discussion, or pull request for a suspected
vulnerability. Use [GitHub private vulnerability reporting](https://github.com/trycua/cua/security/advisories/new)
so the report reaches the maintainers without disclosing details publicly.

Include the smallest amount of information needed to investigate:

- the affected Cua component and version or commit;
- the operating system, environment, and configuration when relevant;
- a concise reproduction or proof of the behavior;
- the security impact and conditions required to trigger it; and
- any known mitigation or workaround.

Do not include credentials, tokens, private user or customer data, or unrelated
sensitive material. Redact logs and screenshots before attaching them. Please
avoid public disclosure until maintainers have coordinated remediation and an
appropriate disclosure timeline with you.

Maintainers will use the private report to acknowledge the finding, request any
missing evidence, coordinate remediation, and discuss attribution. Security
reporter attribution remains private until disclosure is permitted.

To propose or debate a security boundary—such as a permission model, trust
boundary, or public contract that does not disclose an exploitable defect—use
the **Request for comments** issue form and [`rfcs/README.md`](rfcs/README.md).
Use private reporting for a specific exploitable defect in shipped code or
configuration. When design and vulnerability details are entangled, report
privately first; maintainers can open or unblock the public RFC after remediation
is coordinated.

For incorrect behavior without a security impact, use the repository's **Bug
report** issue form instead.
