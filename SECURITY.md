# Security Policy

## Report a Vulnerability

Do not open a public issue, RFC, discussion, or pull request for a suspected
vulnerability. Use [GitHub private vulnerability reporting](https://github.com/trycua/cua/security/advisories/new)
so the report reaches the maintainers without disclosing details publicly.
This is the only reporting channel this repository currently has set up and
verified to reach us; if we later stand up a dedicated mailbox (for example
`security@trycua.com`) and a `security.txt`, this page and the "What the
relay can see" doc will link them here instead of or alongside GitHub.

## Response targets

These are targets, not contractual guarantees:

- **Acknowledgment:** within 2 business days of a private report.
- **Initial assessment** (confirmed, not a vulnerability, or needs more
  information): within 5 business days.
- **Remediation timeline communicated to the reporter:** within 10 business
  days of confirmation, scaled to severity -- a critical, actively
  exploitable issue in a hosted service (for example `relay.cua.ai`) is
  prioritized over one that needs local access or an unlikely
  configuration.
- We will keep the reporter updated at least every 2 weeks until resolution
  or an agreed timeline, whichever is sooner.

## Supported versions

Cua is a monorepo of independently released components (CLI, SDK, Cua
Driver, Cua Spaces apps, the relay, the Keyvault, teleport, and others; see
[`Development.md`](Development.md)). We support and patch the **current
released version of each component**, plus the version actually deployed
for anything we host ourselves (`relay.cua.ai`, `auth.cua.ai`, and other
`*.cua.ai` services): those always run our latest fix once it ships,
independent of what any self-hoster has deployed. We do not commit to
backporting fixes to older tagged releases of self-hosted components; self-
hosters should track the latest release of the component they run.

## Safe harbor

We will not pursue or support legal action against, and consider authorized
under the Computer Fraud and Abuse Act (and equivalent state and
international laws), good-faith security research that:

- is reported to us privately (see "Report a Vulnerability" above) before
  any public disclosure, with a reasonable coordinated-disclosure window;
- avoids privacy violations, destruction of data, and interruption or
  degradation of our services (including `relay.cua.ai` and other hosted
  services) beyond what is strictly necessary to demonstrate the issue;
- does not access, modify, or exfiltrate data that is not your own (an
  account and test machines you control are the right way to demonstrate
  an issue that involves other users' data or devices);
- stops testing and reports immediately upon discovering any of the above
  (unintended access to another user's data, for example), rather than
  continuing to explore; and
- does not use social engineering, physical attacks, or denial-of-service
  techniques against our people, offices, or infrastructure.

If a third party initiates legal action against you for activity conducted
consistent with this policy, we will take reasonable steps to make clear
that your actions were authorized. This safe harbor does not apply to
testing against third-party services we rely on or embed (for example
identity providers, cloud infrastructure, or dependencies) except as their
own policies allow; check their own disclosure terms first.

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

To propose or debate a security boundary (a permission model, trust boundary,
or public contract) that does not disclose an exploitable defect, use the
**Request for comments** issue form and [`rfcs/README.md`](rfcs/README.md).
Use private reporting for a specific exploitable defect in shipped code or
configuration. When design and vulnerability details are entangled, report
privately first; maintainers can open or unblock the public RFC after remediation
is coordinated.

For incorrect behavior without a security impact, use the repository's **Bug
report** issue form instead.
