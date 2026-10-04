# Security policy

## Supported versions

Only the latest commit of the default branch is supported. The project has no release yet.

## Reporting a vulnerability

Report it privately, through the private vulnerability reporting of the repository (the
*Security* tab, then *Report a vulnerability*). Do not open a public issue for it.

That feature must be enabled in the repository settings before the project is published: this
file cannot check it, and until it is enabled there is no private channel.

A report is easier to act on with: the commit it applies to, the configuration used (never the
token), the exact script or request, and what happened compared with what should have.

## What to expect

- An acknowledgement within 7 days.
- A first assessment, saying whether the report is accepted as a vulnerability, within 30 days.
- A fix, or a reason why there will be none, communicated to the reporter before it is made
  public. The reporter is credited unless they ask not to be.

## Good-faith research

Research that follows this policy is welcome and will not be pursued: testing on your own
machine with your own token, reporting what you find, and not keeping, sharing or using
anyone's data or access beyond what is needed to show the problem. Do not run tests against
machines or accounts that are not yours.

## Scope

In scope: the Hub (`libremodular-mcp` and `libremodular-launch`) and the bridges in `bridges/`.
For example: the token reaching an output, a log or a process argument; a bridge answering
someone who does not hold the token, or listening on an address other than `127.0.0.1`; the
Hub opening a connection to anything but a local bridge; a file the Hub creates with
permissions wider than its owner's; a capture or a path escaping its directory.

Not in scope:

- The applications themselves (GIMP, Krita, Blender, Inkscape, LibreOffice) and their scripting
  interfaces. This includes the UNO port of LibreOffice, which has no authentication: the
  README lists it under *Limits*.
- A script that gets past the script guard. The guard is a safety net against accidents, not a
  sandbox, and a script already runs with the rights of the user inside the application.
- Anything that needs the attacker to already run code as the same user: that user can read the
  token file.
