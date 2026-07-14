# Desktop Auto-Update

Tauri shell обновляется через `tauri-plugin-updater`.

Клиент сначала проверяет `/desktop/latest.json` на выбранном сервере. При
сетевой ошибке или ошибке скачивания используется GitHub Releases. Workflow
релиза зеркалирует подписанные артефакты в Docker volume на deploy-сервере.

## Bootstrap (один раз)

```bash
cd src-tauri
cargo install tauri-cli --version "^2" --locked
cargo tauri signer generate -w ~/.tauri/voice-hub.key   # password пустой
```

- `voice-hub.key.pub` → `src-tauri/tauri.conf.json` → `plugins.updater.pubkey` (коммитим).
- `voice-hub.key` → GitHub Secret `TAURI_SIGNING_PRIVATE_KEY`.

## Релиз

Push в `src-tauri/**` → `auto-tag-desktop.yml` бампит patch, тегает, триггерит `release-desktop.yml`.

Minor/major: вручную поднять `version` в `tauri.conf.json` (+ `Cargo.toml`, `Cargo.lock`), запушить — auto-tag увидит untagged версию и тегнёт её как есть.
