# ThodHUB API 1

Windows x64. Portable ZIP. Stable versions `X.Y.Z` (numeric, no leading zeros).
IDs: lowercase letter followed by 1–63 lowercase letters, digits or hyphens.
Public catalog: `ThodProd/ThodHUB-Registry/catalog.json`, branch `main`.

## Standalone
ZIP root contains `manifest.json`:
```json
{"schemaVersion":1,"hubApi":1,"id":"my-app","version":"1.0.0","platform":"win-x64","mode":"standalone","launch":{"executable":"app/MyApp.exe","arguments":[]},"permissions":[]}
```
Package all runtime files under `app/`; no SDK/development runtime required on the end-user PC.
`arguments` is an array, never a shell command. `{dataDir}` expands to the app data directory.
Normal processes receive `THODHUB_DATA_DIR`. Elevated apps declare `requiresElevation:true` and
accept dataDir explicitly, e.g. `["--thodhub-data","{dataDir}"]`, because UAC drops environment.
The hub itself runs asInvoker. Working directory is the executable's directory.
Desktop shortcuts invoke the hub with `--launch <id>`, resolving the installed version at launch.
The app should handle repeated launches. No external installer hooks, services or driver installation.
Do not add persistent OS integration the hub cannot uninstall.

## Embedded
```json
{"schemaVersion":1,"hubApi":1,"id":"my-app","version":"1.0.0","platform":"win-x64","mode":"embedded","ui":"ui/index.html","worker":{"executable":"worker/MyWorker.exe","arguments":[]},"permissions":["settings","worker"]}
```
`worker` is optional. All UI resources are local. Copy `sdk/thodhub.js` beside the page and load it.
Call `await thodHub.call('worker.calculate',{value:42})`.
The UI runs in a sandboxed iframe; do not access parent DOM/native APIs directly.
The host owns navigation, close and fullscreen. Support narrow windows and both color schemes.

| Method | Permission | Input | Output |
|---|---|---|---|
| settings.get | settings | null | saved JSON, initially null |
| settings.set | settings | JSON | true; atomic settings.json write |
| settings.path | settings | null | absolute data directory |
| state.dirty | none | {dirty:true/false} | true; hub 0.1.1+ asks before closing a document with unsaved edits |
| clipboard.write | clipboard.write | {text} | true |
| dialog.folder | dialog.folder | null | absolute selected folder or null |
| files.export | files.export | {name,base64} | {canceled,filePath?}; save dialog, <=8 MiB |
| worker.method | worker | JSON | worker result or rejected Promise |

Unknown bridge methods/permissions are rejected. Experimental document.pdf/document.print are not a supported integration surface in 0.1.0.
Worker protocol: UTF-8, newline-delimited JSON-RPC 2.0, **compact single-line JSON**, string IDs.
Input: `{"jsonrpc":"2.0","id":"abc","method":"calculate","params":{"value":42}}`.
Output: `{"jsonrpc":"2.0","id":"abc","result":{"value":84}}` or
`{"jsonrpc":"2.0","id":"abc","error":{"code":-32000,"message":"Explanation"}}`.
The worker. prefix is stripped. Log to stderr, never stdout. Request timeout 30s, line limit 8 MiB.
Timeout does not cancel internal work: implement cancellation/progress for long tasks.
Worker receives THODHUB_API=1 and THODHUB_DATA_DIR; exit on stdin EOF. Host waits 2s, then terminates the process tree.
Workers remain while switching pages; stop on update/uninstall/hub close.

## Releases and registry
Run `scripts/package-module.ps1 -PackageDirectory <stage> -Repository owner/repo -OutputDirectory <out> -Notes 'Changes'`.
Publish its ZIP and `thodhub-release.json` as assets of stable `hub-vX.Y.Z` (not draft/prerelease).
Descriptor: version, packageUrl, sha256 (64 hex), size (bytes), notes, platform:win-x64, hubApi:1.
The hub examines the newest 10 releases and uses the first stable release containing the descriptor.
Package URL must be `https://github.com/owner/repo/releases/download/...` in that exact repository.
Do not replace released assets; publish a new version. Maximum: 1 GiB archive, 2 GiB extracted, 20,000 entries.
No links, traversal, reserved Windows names, duplicate paths or alternate streams.

Catalog entry:
```json
{"id":"my-app","name":"My App","kind":"Редактор","description":"Редактирование локальных документов.","repository":"ThodProd/MyApp","category":"Работа","icon":"library","iconUrl":"https://raw.githubusercontent.com/ThodProd/ThodHUB-Registry/main/assets/icons/my-app.svg","mode":"standalone","artworkUrl":"https://raw.githubusercontent.com/ThodProd/ThodHUB-Registry/main/assets/my-app.svg"}
```
Catalog has schemaVersion:1 and an apps array. Modes must match manifest. Icons: library, resume, mixer, vpn, monitor, copy, code.
Artwork must be an HTTPS raw.githubusercontent.com URL. See `registry/` and `schemas/`.

## Data and lifecycle
Installation: Apps/id/version; data: Data/id, outside versions. Updates preserve the previous version for rollback.
Rollback does not reverse data migrations: keep migrations backward-compatible or back up before migrating.
Uninstall removes all versions and the managed desktop shortcut. Data stays unless the checkbox is selected.
Running programs/open files block deletion. Never store user data next to executables or secrets on GitHub.
Protect secrets with Windows-protected storage and document backup needs.
Examples: [CopyAll](https://github.com/ThodProd/CopyAll/tree/main/hub) (.NET embedded),
[ThodVPN](https://github.com/ThodProd/ThodVPN/tree/master/hub) (Python standalone).

## Context menu and cleanup (API 1)
An optional `cleanup` has the same shape as launch: an executable inside the package and argument array.
Example: `"cleanup":{"executable":"worker/CopyAll.Worker.exe","arguments":["--uninstall-context"]}`.
It runs without elevation before uninstall, with a 15s timeout. A nonzero exit stops removal.
Only remove integration keys carrying your own ownership marker. Use HKCU/Software/Classes for per-user menus.
THODHUB_LAUNCHER points to the hub executable. Context menu commands should invoke that stable launcher:
standalone: `"<hub>" --launch <id> --target "%1"` (extra args forwarded as an array),
clipboard worker: `"<hub>" --invoke <id> context.copy "%1"` (worker result must contain clipboardText; permission clipboard.write required).
Use `%V` for a directory-background menu. This resolves the installed version after updates.
Rollback cleans up current context-menu integration; enable it again in the selected version if supported.
Do not add services/drivers or machine-wide registry hooks that an unelevated cleanup cannot remove.

## Metadata, backup and release workflow
New catalog entries include short Russian `kind`, Russian `description`, and a landscape `artworkUrl` per APP-STYLE.md.
Each app must have its own square `iconUrl` (HTTPS raw.githubusercontent.com), separate from artworkUrl.
Embed a multi-size ICO in the executable and use it for the window/taskbar/desktop shortcut; see APP-STYLE.md.
Standalone manifests may declare up to 8 `actions`, e.g. `[{"id":"default-viewer","label":"Сделать просмотрщиком по умолчанию","arguments":["--default-viewer"]}]`.
These buttons launch the installed executable with the declared extra argument array, never a shell command.
File associations must resolve via THODHUB_LAUNCHER --launch id; register supported formats then open Windows Default Apps.
Windows requires confirmation in system UI: do not alter protected UserChoice hashes or claim silent association changes.
All permanent configuration belongs in THODHUB_DATA_DIR and is included in dated backup snapshots.
Read BACKUPS.md for excluded transient directories; never store required config in those directories.
Read RELEASE-CHECKLIST.md for version updates, size/hash, BOM and package path compatibility.

## Windows integration (ThodHUB 0.1.2+)
Optional `integration: {executable:"app/App.exe", arguments:["--register-integrations"]}` runs after installation/update and after rollback, without elevation. Mandatory paired `cleanup` removes only owned per-user registrations. Both must exit within 15s, exit 0 only on success, write errors to stderr, and be idempotent. The install hook registers availability (Open With/context menus), never silently overrides Windows UserChoice. Do not download dependencies or start persistent workers inside the hook. An integration failure is displayed separately from installed version and can be retried from app details.
Actions that complete quickly can declare `waitForExit:true`; the hub waits and reports nonzero exit codes instead of claiming success on process start. Normal standalone launches remain asynchronous.
Shortcuts for either mode use `--launch <id>`: standalone launches the installed EXE; embedded activates the existing hub via a per-user named pipe and opens the module. Never hardcode a version folder in a shortcut or shell command. Integration must resolve through THODHUB_LAUNCHER.
Embedded resources use an origin specific to id+version, so updated HTML, CSS and JS cannot come from the previous version's cache.
