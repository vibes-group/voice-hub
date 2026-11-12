<p align="center">
  <img src="docs/logo.svg" alt="voice-hub" width="128" height="128">
</p>

# voice-hub

[![Latest release](https://img.shields.io/github/v/release/vibes-group/voice-hub?label=release&color=blue)](https://github.com/vibes-group/voice-hub/releases/latest)
[![Release Desktop](https://github.com/vibes-group/voice-hub/actions/workflows/release-desktop.yml/badge.svg)](https://github.com/vibes-group/voice-hub/actions/workflows/release-desktop.yml)
[![License](https://img.shields.io/github/license/vibes-group/voice-hub)](LICENSE)

Постоянная голосовая комната для своей компании. Self-hosted: один сервер со встроенным TURN, без внешних сервисов.

## Возможности

- **Голос** с шумоподавлением.
- **Демонстрация экрана.**
- **Чат** с вложениями.
- **Desktop-клиент для Windows** — глобальные хоткеи, автообновление.
- **Вход по паролю** — без аккаунтов и регистрации.

## Скачать

⬇ **[Последний релиз для Windows](https://github.com/vibes-group/voice-hub/releases/latest)**

Сборка универсальная: адрес сервера вводится при первом запуске.

В релизах также публикуется portable ZIP **GPUI Beta** для Windows x86_64:
нативные голосовые комнаты с TURN, эхоподавлением и теми же RNNoise/FastEnhancer,
чат и вложения, демонстрации экрана, настройки участников, администрирование,
трей, глобальные клавиши и подписанные обновления с вашего сервера.
Профиль GPUI хранится отдельно; сессия защищена Windows Credential Manager.

## Разработка

```bash
docker compose -f docker-compose.dev.yml up -d --build
```

Приложение доступно на `http://localhost:8080`, пароль `dev`.

Фронтенд с HMR — отдельно от контейнера:

```bash
docker compose -f docker-compose.dev.yml up -d app --build
cd frontend && npm install && npm run dev
```

Desktop-клиент (`src-tauri/`) локально:

```bash
cd src-tauri
cargo install tauri-cli --version '^2'
cargo tauri dev
```

GPUI beta (`src-gpui/`), Windows с Visual Studio Build Tools и Windows SDK:

```bash
python scripts/check-cargo-age.py src-gpui/Cargo.lock
python scripts/prepare-webrtc.py --platform windows-x86_64 --github-env src-gpui/.native/build.env
python scripts/prepare-decoder.py --github-env src-gpui/.native/build.env
# Set LK_CUSTOM_WEBRTC and APP_DECODER_ARCHIVE from src-gpui/.native/build.env in your shell.
cd src-gpui
cargo run --locked
```

## Лицензия

[MIT](LICENSE)
