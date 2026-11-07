# Security reporting

Rustic is an experimental foundation, not a browser suitable for sensitive or
arbitrary web browsing. There are no released versions or supported production
security-update branches yet. Development fixes target `main` until a release
policy is established.

If GitHub's **Report a vulnerability** option is available under this repository's
Security tab, use it for a private report. If it is unavailable, open a public
issue containing only a request for a private reporting channel; do not include
exploit details, credentials, personal data or a working sensitive reproducer.
The maintainer must establish a verified private contact before receiving details.
No email address, response deadline, bounty or private-reporting setting is
invented or promised by this file.

Once a private channel is available, include the affected commit/platform, impact,
minimal reproducer, expected/actual behavior and any diagnostic evidence with
sensitive data removed. See [the threat model](docs/SECURITY.md) for security
boundaries and the gates required before general remote browsing.
