# Security policy

## Supported versions

Only the latest release line receives security fixes.

## Reporting a vulnerability

Email the maintainers via the GitHub "Report a vulnerability" flow on the
Security tab, or open a private security advisory. Please do not open public
issues for exploitable bugs.

## Scope

clipLine stores everything you copy — including passwords — in a local,
unencrypted SQLite database by default (matching the original app). Treat the
database file as sensitive. Encryption (SQLCipher) is on the roadmap.

Special considerations:

- Clipboard contents from password managers often land in history. Exclude
  those apps once app exclusion rules ship; until then, use your password
  manager's "clear clipboard on exit" option.
- Paste injection simulates keystrokes. A malicious foreground window can
  receive injected Ctrl+V; only paste into windows you trust.
