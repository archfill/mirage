# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in Mirage, please report it responsibly.

**Do NOT open a public GitHub issue for security vulnerabilities.**

Instead, please use [GitHub Private Vulnerability Reporting](https://github.com/archfill/mirage/security/advisories/new) to submit your report.

### What to include

- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

### Response timeline

- **Acknowledgment**: within 48 hours
- **Initial assessment**: within 1 week
- **Fix or mitigation**: depends on severity, but we aim to address critical issues as quickly as possible

## Scope

Mirage handles sensitive data including:

- Cloud storage credentials (passwords, tokens)
- File contents downloaded from remote servers
- Local filesystem operations via FUSE

Security issues in any of these areas are in scope.

## Supported Versions

| Version | Supported |
| ------- | --------- |
| 0.1.x   | Yes       |
