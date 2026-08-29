# Security policy

## Supported versions

Security fixes are provided for the latest Archi release only.

| Version | Supported |
| --- | --- |
| 0.5.x | Yes |
| 0.4.x and earlier | No |

The public [security design](docs/SECURITY_DESIGN.md) describes Archi's threat
model, extraction pipeline, implemented controls, and explicit limitations.

## Reporting a vulnerability

Please report suspected vulnerabilities through
[GitHub's private vulnerability reporting](https://github.com/gg333/archi/security/advisories/new).
Do not open a public issue or publish exploit details before a fix is available.

Include, when possible:

- the Archi version, macOS version, and Mac architecture;
- the affected archive format and operation;
- clear reproduction steps and the expected security impact;
- a minimal test archive containing no personal or confidential data; and
- relevant logs with passwords, usernames, and local paths removed.

Archive path traversal, unsafe links, unintended file replacement, command
injection, password disclosure, arbitrary code execution, preview safety
bypasses, and signing or update-integrity problems are particularly useful to
report. If a problem originates in a bundled dependency such as 7-Zip, please
still report it when it affects Archi users.

We aim to acknowledge reports within five business days and provide an initial
assessment within ten business days. Remediation and disclosure timing depend
on severity and whether coordination with an upstream project is required.

Archi does not currently operate a vulnerability-reward or bug-bounty program.
