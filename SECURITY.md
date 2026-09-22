# Security Policy

## Supported versions

Only the latest release receives security fixes.

## Reporting a vulnerability

Do not open a public issue containing exploit details or user data. Report the
problem privately to the repository maintainers through GitHub's private
security advisory feature. Include affected versions, reproduction steps,
impact and any suggested mitigation.

Maintainers should acknowledge a report within seven days, keep the reporter
informed, and publish a coordinated fix and advisory when practical.

## Security boundaries

Sylphra is a document-focused browser. Its built-in JavaScript engine
does not provide a complete DOM or the isolation guarantees of a mature
multi-process browser. Do not use it for high-risk authentication, payments or
untrusted active web applications.

Untrusted HTML and PDF preparation runs in a short-lived renderer worker with
bounded request/response frames, checked decompression, parser limits and a
15-second timeout. This crash boundary is not a Windows AppContainer or
low-integrity sandbox, so it reduces shell crashes but must not be described as
complete operating-system sandboxing.

Local subresources are restricted to the selected document's directory. The
PDF reader rejects encrypted files and enforces byte, object, page, stream and
text limits. It does not execute embedded PDF actions or JavaScript.

The experimental password-store module uses reversible obfuscation and is not a
supported password manager. It is excluded from the user interface until
an operating-system credential vault is used.
