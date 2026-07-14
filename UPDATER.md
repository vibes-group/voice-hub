# Desktop Auto-Update

Tauri shell обновляется через `tauri-plugin-updater`.

Клиент проверяет `/desktop/latest.json` только на выбранном сервере. Workflow
релиза зеркалирует подписанные артефакты из GitHub Releases в Docker volume на
deploy-сервере. GitHub остаётся доступен только для ручного скачивания.

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
