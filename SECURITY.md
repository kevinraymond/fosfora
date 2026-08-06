# Security Policy

## Supported Versions

Security fixes are applied to the latest release. Older versions are not
maintained — please update before reporting an issue.

| Version          | Supported |
| ---------------- | --------- |
| 1.16.x (latest)  | ✅        |
| < 1.16           | ❌        |

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

Instead, report them privately through GitHub's private vulnerability reporting:

➡️ **[Report a vulnerability](https://github.com/kevinraymond/fosfora/security/advisories/new)**

Please include as much of the following as you can:

- The type of issue and the component affected (e.g. OSC handler, web touch
  surface, media/webcam decoding)
- Steps to reproduce, or a proof-of-concept
- The version and platform (OS + GPU) you observed it on
- Any relevant log output (`RUST_LOG=fosfora=debug`)

You can expect an initial response within about a week. We'll keep you updated
as we investigate and, if a fix is warranted, coordinate a release.

## Scope

Fosfora is a **local, live-performance tool**, not a hardened network service.
By design it opens several local interfaces that you should keep on trusted
networks. Reports about the following are in scope:

- **OSC in/out** — UDP control surface (default port 9000)
- **Web touch surface** — the built-in HTTP/WebSocket server used to control the
  app from a phone or tablet on the local network
- **NDI®** — video output over the local network
- **AI shader assistant** — the API key stored in the OS keyring and requests
  made to the user-configured LLM endpoint
- **Media decoding** — parsing of untrusted image/GIF/video files loaded as
  media layers

Denial of service that requires access to the same local network as the
performer, and issues that only arise from intentionally hostile local
configuration, are lower priority — but we still welcome the report.
