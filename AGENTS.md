# ThodHUB integration
Read docs/MODULE-CONTRACT.md and docs/RELEASE-CHECKLIST.md before changing integration.
For a new task read docs/AGENT-PROMPT.md and wait for the user's actual task; do not guess an earlier project.
Follow docs/APP-STYLE.md for all user-facing UI, descriptions and catalog artwork.
Keep heavy workloads in separate processes. Preserve user changes and data. Never publish credentials/configs.
Follow the user's chosen test mode; do not generate screenshots or broad self-tests unasked.
Always build and validate the package manifest, paths, version, size and hash before publishing.
For core installer/backup changes run the relevant tests: dotnet run --project tests/ThodHub.Tests.
State untested functional/hardware/network scenarios honestly. New versions need a new immutable release.
