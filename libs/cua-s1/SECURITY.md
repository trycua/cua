# Cua-S1 security

## Report a vulnerability

Do not open a public issue, discussion, or pull request for a suspected
vulnerability. Use
[GitHub private vulnerability reporting](https://github.com/trycua/cua/security/advisories/new)
and identify Cua-S1 plus the affected checkpoint, package version, or commit.

Provide the smallest reproduction needed to investigate, the security impact,
and any known mitigation. Do not include credentials, private user data, or
unrelated sensitive material. Redact logs and screenshots before attaching
them, and avoid public disclosure until remediation and disclosure timing have
been coordinated with the maintainers.

For incorrect behavior without a security impact, use the repository's public
bug-report process.

## Deployment risks

A computer-use model acts on content supplied by applications and may encounter
malicious instructions, deceptive controls, or unexpected state. Treat screen
content, documents, web pages, and model-generated actions as untrusted.

Operators should:

- isolate the runtime from the host and unrelated accounts;
- grant only the credentials, applications, files, network destinations, and
  actions required for the evaluated task;
- keep secrets out of observations and logs where possible;
- validate targets and state before actions and independently verify outcomes;
- require human confirmation for consequential, irreversible, privileged, or
  external actions;
- enforce time, action, and resource limits and provide an immediate stop
  mechanism; and
- retain enough redacted evidence to investigate failures without collecting
  unnecessary sensitive data.

Treat all tool results, planning reports, MCP stdio output, and runtime logs as
sensitive. They can contain PDF-extracted fields, form labels, window metadata,
and values selected for entry. Do not send them to shared telemetry or retain
them in unredacted logs.

The `cua-s1-form-v0` name denotes a specialist research checkpoint, not a
security boundary. Its outputs must not be used as authorization or as proof
that a requested operation is safe.

## Artifact integrity

Obtain checkpoints and packages only from documented project distribution
channels. Verify the exact version and any published digest or signature before
use. Do not load untrusted model or serialization artifacts into a privileged
process. Cua-S1 accepts `safetensors` plus JSON checkpoints and rejects common
pickle-based checkpoint extensions, but callers must still verify the source
and integrity of every artifact.
