# Security reporting

Aether is a research prototype. Its implemented software invariants and open
boundaries are described in [docs/SECURITY.md](docs/SECURITY.md). No release is
represented as production certified or hardware-isolation validated.

For a suspected vulnerability, use GitHub's **Report a vulnerability** option
on this repository's Security tab if private reporting is enabled. Include the
commit SHA, affected boundary, environment, minimal reproducer, expected and
actual behavior, and relevant logs. Do not include credentials or customer data.
If private reporting is unavailable, open an issue requesting a private contact
without exploit details. Maintainer response times are not currently guaranteed.

Current scope is the latest main research code; historical versions have no
separate security maintenance commitment. RustSec dependency audit and host tests are
engineering checks, not an independent security audit. Maintainers should enable
private vulnerability reporting and require CI checks through repository settings.
