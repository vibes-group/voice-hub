# Rules for AI agents

Repository is **public**. Every commit is visible.

## Never commit

- Real names, emails, phones, messenger handles
- Absolute home paths — use `~` or relative
- Real hosts / IPs of prod/dev servers — use placeholders
- `.env` contents, tokens, keys, passwords
- Output of `whoami`, `hostname`, `id`, `env`

## Doc style

- README, release notes, comments — signal only. Don't explain the obvious.
- No planning `*.md` files in the repo. Plans live in PRs/tickets.
- Before PR: grep for «not yet» / «TODO» on shipped features.

## Env naming

| Tier | Prefix |
|------|--------|
| App-level | `APP_*` |
| Subsystem | `<DOMAIN>_*` (≥2 vars) |
| Infra context | no prefix (`IMAGE_TAG`, `PUBLIC_IP`) |

Range pairs: `<NAME>_MIN` / `<NAME>_MAX`. Required vars have no defaults, crash on startup. Secrets only via env.

## Deploy (server)

- Infra lives in `vibes-group/infra`: push to `master` → `build.yml` builds and pushes `ghcr.io/vibes-group/voice-hub-app:<sha>` → infra's reusable `deploy.yml` rolls it out.
- Ports: HTTPS/WSS → Caddy → `voice-hub-app:8080`; UDP `3478`, `10101-10200`, `49160-49199` go straight to the app (TURN + media).
- Secrets: `DEPLOY_HOST` / `DEPLOY_SSH_KEY` / `DEPLOY_HOST_KEY` (org — infra reusable workflow), `VOICE_HUB_HOST` (org — public domain, becomes `APP_HOSTNAME`), `APP_ADMIN_PASSWORD` (repo), `TAURI_SIGNING_PRIVATE_KEY` (repo — desktop release signing).

## Desktop releases

- Clients update via `tauri-plugin-updater` from `/desktop/latest.json` **on their own server**: the release workflow mirrors signed artifacts from GitHub Releases to the deploy server; GitHub stays manual-download only.
- Push to `src-tauri/**` → `auto-tag-desktop.yml` bumps patch, tags, triggers `release-desktop.yml`. Minor/major: bump `version` in `tauri.conf.json` (+ `Cargo.toml`, `Cargo.lock`) and push — auto-tag tags the untagged version as is.
- Signing key (bootstrap, once): `cargo tauri signer generate -w ~/.tauri/voice-hub.key` (empty password); pubkey → `plugins.updater.pubkey` in `tauri.conf.json` (committed), private key → secret `TAURI_SIGNING_PRIVATE_KEY`.

## Git

- Conventional commits (`feat:`, `fix:`, `chore(desktop):`) — `release-desktop.yml` parses the prefix.
- No `--amend` on published commits, no force-push to master.
- Don't tag manually — auto-tag bot handles it.
- Workflow triggers use positive `paths`, never `paths-ignore` — a forgotten path fails loudly instead of causing silent extra runs and auto-tag loops.
