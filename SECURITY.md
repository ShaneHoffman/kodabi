# Security policy

Thanks for helping keep Kodabi and its users safe.

## Reporting a vulnerability

**Report privately through GitHub, not in a public issue.**

Open a private report here:
[**Report a vulnerability**](https://github.com/ShaneHoffman/kodabi/security/advisories/new).
That form is GitHub's private vulnerability reporting, so the details stay visible only to you and
the maintainer until a fix is released.

Please include, as far as you can:

- the Kodabi version affected (Settings, then About, shows the build you are running),
- what an attacker can do, and what access they need to do it,
- the steps to reproduce it, and
- anything you already know about a fix or a workaround.

Kodabi is maintained by one person, so there is no guaranteed response time. Expect a best-effort
acknowledgement, and a follow-up once the report has been looked at properly. If a report turns out
to be valid, you will be credited in the advisory unless you would rather not be.

Please do not open a public issue, post a proof of concept publicly, or test against anyone else's
machine or data before the fix ships.

## Supported versions

Kodabi is pre-alpha and ships as a self-updating Windows app, so security fixes go out in the next
release rather than as backports.

| Version | Supported |
| ------- | --------- |
| 0.2.x (latest release) | Yes |
| Older releases | No |

The installed app checks for updates on startup and offers them with a click, so "update to the
latest release" is the supported way to receive a fix. Installers and updates are code-signed, and
updates additionally carry a minisign signature the app verifies before installing.

## Scope

Kodabi is a local-first desktop app. It has no server, no account system, and no telemetry, and your
notes and recordings stay on your machine. The surfaces most worth your attention are:

- the installer and the auto-updater, including their signature checks,
- `kodabi-mcp`, the bundled MCP server, and what it exposes to a connected client,
- how the app handles local files: the vault, the note index, settings, and recordings,
- the models the app downloads on first run, and how their integrity is verified.

Out of scope: vulnerabilities in Windows, WebView2, the `claude` CLI, or other third-party
dependencies, unless Kodabi's own use of them is what creates the problem. Report those upstream, and
tell us if Kodabi needs to change too.

Because Kodabi records meetings, a bug that captures audio without the recording being visible in the
app, or that keeps audio past the retention policy, counts as a security issue. Please report it here
rather than as a normal bug.
