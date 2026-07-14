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

## Лицензия

[MIT](LICENSE)
