@AGENTS.md

# This fork

`ransleygonsalves-beep/figcraft` is a fork of `storytold/figcraft` for our own team. The rules in
AGENTS.md still apply; this section adds what is particular to the fork. Keep it and the fork
section at the top of README.md up to date when a feature, branch or release step changes.

## Remotes and branches

- `fork` is our repository (push here); `origin` is upstream `storytold/figcraft` (fetch only;
  never push to it).
- `main` follows upstream. Each feature is a `feat/*` (or `fix/*`) branch from `main`, so it can
  go upstream on its own.
- `combined` merges the feature branches; `designer` merges `combined` and is what ships. Fork-only
  files (this section, README's fork section, `.github/workflows/designer-release.yml`) live on
  `combined`/`designer`, never on a feature branch.
- After a merge, check for conflict markers before committing (`git grep -l -E '^(<<<<<<<|>>>>>>>) '`):
  `crates/ui-egui/locales/en.toml` conflicts often when two branches add strings; keep both
  sides (the catalog test then passes).

## Releases for the designer

1. On `designer`: `cargo xtask version set X.Y.Z`, commit.
2. Tag `vX.Y.Z` and push the branch and the tag to `fork`.
3. `.github/workflows/designer-release.yml` builds unsigned Windows (`.msi`, portable `.zip`), a
   universal macOS `.dmg` and `figcraft-cli` zip (macos-15 runner), the browser build
   (`figcraft-web-<version>.zip`) and the Linux server
   (`figcraft-server-<version>-linux-x86_64.tar.gz`) into a draft release with `SHA256SUMS.txt`;
   publish it on GitHub.

The in-app updater (`UPDATE_APP` in `crates/ui-egui/src/lib.rs`) points at the fork; unsigned
builds get an "update available" notice, not an automatic install. Ask before pushing a tag or
publishing a release.

## What the fork adds

| Feature | Where |
|---|---|
| Team accounts: sign-in, invites, per-file access by email | Server: `apps/figcraft-server/src/accounts.rs`, `store/members.rs`. Protocol: `AccountReq`/`AccountReply` in `crates/collab/src/message.rs`. Engine: `account.*` in `crates/engine/src/sharing/accounts.rs`. UI: `crates/ui-egui/src/home/accounts.rs` |
| Version history on the server, with who edited | Server: `apps/figcraft-server/src/store/versions.rs`. Engine: `account.versions`, `account.saveVersion`, `account.previewVersion`, `account.restoreVersion`… UI: `crates/ui-egui/src/panels/versions.rs` (server history for files from your team's server) |
| Browser sign-in | `crates/collab-net/src/exchange.rs` (`Exchange::account`), `apps/figcraft-web/src/kept.rs` (sessions in local storage) |
| Agent tokens, `figcraft-cli account login` | Server: `apps/figcraft-server/src/accounts.rs` (`create_agent`, `agent_of`), `store/backend.rs` (`agent_may`). CLI: `apps/figcraft-cli/src/account.rs`. Engine: `accounts::use_token` |
| Admin panel (People, Invites, Files, Activity), Change password | `crates/ui-egui/src/home/accounts.rs` (`people` and its tabs); `account.activity` reads the audit log (`Store::activity`) |
| Password reset, audit log, team list off with accounts only | `accounts.rs` (`reset_code`, `reset`), `apps/figcraft-server/src/audit.rs`, `store/team.rs` (`team_access`) |
| Vectorize image, Remove background | `feat/vectorize-image`, `feat/remove-background` |
| Smooth tool (vector edit mode) | `feat/vector-smooth-tool` |
| Several PDFs as one file (export asks) | `feat/pdf-one-file`: `export_panel::export_layers` (`PdfChoice`), `dialogs/pdf_choice.rs` |
| Perspective effect (tilt a layer) | `feat/perspective-effect`: `figcraft_doc::Perspective`, `crates/render/src/perspective.rs` (draw flat, then warp) |
| Presenter scroll/hover fixes, opens at 100% | `fix/present-scroll-hover-actual` |
| Video: place MP4/MOV, play, Export as MP4 | `feat/video` (from `feat/remove-background`): `crates/video` (MP4 boxes; `Player` through the system decoder, else Cisco's OpenH264; `Writer`), `crates/render/src/video.rs` (frames in image fills, presenter clock), `crates/engine/src/video_export.rs` (codec download, export job), `cmd/file/video.rs`, `ui-egui/src/dialogs/mp4_export.rs` |
| Water reminder (easter egg: blue after an hour, water card) | `feat/water-reminder`: `crates/ui-egui/src/water.rs` (clock, `tint`, the card and its water), off switch at the bottom of Help ▸ About (`pref.waterReminder`); kept out of the README on purpose; `ui.set {waterDue}` shows it at once |

User-facing docs for accounts: `docs/server.md#accounts-and-version-history`.

### Accounts: things to keep

- The collab protocol is postcard (positional): add enum variants only at the end and never add
  fields to an existing message; announce new reading abilities with a `client_version` word.
- A session token is a password in effect: it is kept in the preferences, never shown in what
  commands return, and links shown hide it (`hide_account_token`). Passwords are never stored by
  the app.
- An account joins a file with the token `acct-<session>`; the server's `admit` checks it, and the
  hub uses the account's name, not the name the app sends.
- Version authorship comes from the names of the people whose changes the session accepted
  (`note_editors`); checkpoints happen in `changed` (every 30 minutes), `idle` (everyone left) and
  `stop`.
- An agent token is limited by the server, not the client: `agent_may` decides what it may ask,
  `admit` caps its access and names it "<label> for <name>". Never print a token or write one to
  the audit log; the CLI keeps it only in the owner-only settings file.
- Release builds read the repository variable `FIGCRAFT_DEFAULT_SERVER` (the team's `wss://` URL);
  unset, they have no default server rather than the public one.

### Video: things to keep

- Playback uses the operating system's decoder (Windows Media Foundation, macOS AVFoundation): what
  browsers use, so B-frames and HEVC play. Cisco's OpenH264 is for writing MP4s, and plays only
  H.264 without B-frames (the Linux fallback). FFmpeg is out (LGPL/GPL isn't on the licence list).
- **The fork's one exception to `unsafe_code = "forbid"`:** `crates/video` denies it instead, and
  only `src/system_windows.rs` and `src/system_macos.rs` allow it (`#![allow(unsafe_code)]`), each
  `unsafe` block with a `// SAFETY:` note. Keep it there; nothing else in the fork uses `unsafe`.
- Cisco's library is downloaded from Cisco only after the user allows it in the app (no command
  allows it); each platform's `.bz2` SHA-256 is pinned in `video_export::codec::BUILD`, and the
  bindings load only builds they know. Its notice must show where the download is offered:
  "OpenH264 Video Codec provided by Cisco Systems, Inc."
- The macOS decoder has only been compiled here, not run: check it on a Mac.

## Testing notes

- The server's tests start real servers: `cargo test -p figcraft-server`. If a running
  `figcraft-server.exe` locks `target/debug`, use another target directory
  (`CARGO_TARGET_DIR=target/srvtest`).
- `figcraft-collab-net`'s `code_tests::a_share_on_every_interface_names_an_address_others_reach`
  hangs on this machine on `main` too (network setup); skip it with `-- --skip a_share_on_every_interface`.
- To try account features in the app: start a server with a scratch `--config` (its own `data`
  folder and port), make an invite with `figcraft-server invite --admin --config …`, run
  `figcraft --control <port>` and drive it with `engine.execute` (`account.join`,
  `account.publish`, …) and `ui.screenshot`. Injected clicks need move, down, up and move as
  separate `ui.input` calls.
- Don't use real people's emails or names in tests or fixtures; use `example.com` addresses.
- Video tests that need the codec read `FIGCRAFT_OPENH264` (the path to Cisco's library, e.g.
  `openh264-2.6.0-win64.dll` unpacked from `http://ciscobinary.openh264.org/openh264-2.6.0-win64.dll.bz2`)
  and skip without it. ffmpeg's `testsrc2` makes synthetic test videos (`-bf 3` for B-frames,
  `libx265 -tag:v hvc1` for HEVC) to compare frames against.
