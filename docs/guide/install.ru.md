---
title: "Установка termchat.app: Homebrew, одна команда, Windows, Android"
description: "Как установить зашифрованный мессенджер termchat.app на macOS, Linux, Windows и Android, обновить его и проверить, что загрузка подлинная."
---

# Установка

English version: [install.md](install.md).

termchat.app — это три программы, которые открывают учётную запись одного и того же вида:

- `termchat` для терминала;
- `termchat-gui`, приложение для компьютера, для тех, кому удобнее окно;
- приложение для Android.

Выберите нужное. Одна учётная запись открывается в одной программе за раз.

## Клиент для терминала

**Homebrew** (macOS, Linux):

```sh
brew install prophecy1345/tap/termchat
```

**Одна команда** (macOS, Linux). Скачивает сборку для вашего компьютера, сверяет её с `SHA256SUMS` релиза и кладёт `termchat` в `~/.local/bin`:

```sh
curl -fsSL https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.sh | sh
```

Если `~/.local/bin` нет в `PATH`, скрипт напечатает строку, которую нужно добавить в `~/.zshrc` или `~/.bashrc`. Другая папка: `TERMCHAT_INSTALL_DIR=/some/folder` перед `sh`.

**Одна команда** (Windows, в PowerShell). Ставит в `%LOCALAPPDATA%\Programs\termchat` и добавляет эту папку в `PATH`:

```powershell
irm https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.ps1 | iex
```

Затем откройте новый терминал и запустите `termchat`. `termchat --version` показывает версию.

## Приложение для компьютера

Та же учётная запись и то же шифрование в окне, для macOS, Linux и Windows.

```sh
# macOS: termchat.app ставится в ~/Applications.
# Linux: termchat-gui ставится в ~/.local/bin, с пунктом в меню.
curl -fsSL https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.sh | TERMCHAT_GUI=1 sh

# или через Homebrew
brew install prophecy1345/tap/termchat-gui
```

```powershell
# Windows: рядом с termchat.exe, с пунктом в меню «Пуск»
$env:TERMCHAT_GUI = 1; irm https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.ps1 | iex
```

Приложение не подписано платным сертификатом Apple или Microsoft. Установленное так, как выше, оно открывается сразу. Если скачать его браузером:

- macOS откажется запускать его в первый раз. Откройте «Системные настройки» → «Конфиденциальность и безопасность» и один раз нажмите «Всё равно открыть».
- Windows SmartScreen тоже спросит один раз: «Подробнее» → «Выполнить в любом случае».

На Linux приложению нужен glibc 2.28 или новее (Debian 10, Ubuntu 20.04, RHEL 8) и X11 или Wayland.

Окно показывает остальному компьютеру больше, чем терминал: его видят скриншоты и запись экрана, а текст проходит через оконную систему и драйвер видеокарты.

## Android

Android 8.0 или новее; сервисы Google не нужны.

1. Скачайте `termchat-<версия>-android-arm64.apk` со страницы [релизов](https://github.com/prophecy1345/termchat/releases).
2. Откройте его. Android один раз попросит разрешить установку из браузера.

Когда выходит новая версия, приложение показывает отметку в списке чатов. Сборка для F-Droid на подходе, с той же подписью.

## Вручную

Скачайте архив для своего компьютера со страницы [релизов](https://github.com/prophecy1345/termchat/releases). В каждом — `termchat` и `termchat-relay`, сервер.

| Ваш компьютер | Архив |
|---|---|
| Mac на Apple Silicon (M1 и новее) | `termchat-<версия>-aarch64-apple-darwin.tar.gz` |
| Mac на Intel | `termchat-<версия>-x86_64-apple-darwin.tar.gz` |
| Linux, 64-битный ПК | `termchat-<версия>-x86_64-unknown-linux-musl.tar.gz` |
| Linux на ARM (Raspberry Pi 4/5, ARM-серверы) | `termchat-<версия>-aarch64-unknown-linux-musl.tar.gz` |
| Windows 10/11 | `termchat-<версия>-x86_64-pc-windows-msvc.zip` |

- **Linux:** `tar -xzf termchat-*-linux-musl.tar.gz`, затем `./termchat` в этой папке. Файл статический: ничего доустанавливать не нужно.
- **macOS:** `tar -xzf termchat-*-apple-darwin.tar.gz`, затем `./termchat` в этой папке. Если macOS пишет, что программу «нельзя открыть, так как не удалось проверить разработчика» (так бывает с файлами, скачанными браузером), один раз выполните `xattr -d com.apple.quarantine termchat`.
- **Windows:** распакуйте `.zip` и дважды щёлкните `termchat.exe`; лучше всего он выглядит в Windows Terminal. SmartScreen может предупредить о неизвестном издателе: «Подробнее» → «Выполнить в любом случае».

## Обновление

- Homebrew: `brew update && brew upgrade termchat` (и `termchat-gui`). Без `brew update` Homebrew может до суток не видеть новую версию.
- Одна команда: выполните её ещё раз.
- Android: поставьте новый APK поверх старого; данные сохранятся.

Приложения сообщают, когда на relay стоит версия новее вашей.

## Как проверить, что загрузка подлинная

Сделайте это до первого запуска, если для вас это важно.

1. **Контрольная сумма.** В папке со скачанными файлами и `SHA256SUMS` релиза:

   ```sh
   shasum -a 256 -c SHA256SUMS --ignore-missing    # macOS
   sha256sum -c SHA256SUMS --ignore-missing        # Linux
   ```

   Для каждого скачанного файла должно быть `OK`.

2. **Происхождение.** С помощью GitHub CLI:

   ```sh
   gh attestation verify termchat-<версия>-<платформа>.tar.gz --repo prophecy1345/termchat
   ```

   Это доказывает, что файл собран процессом выпуска этого репозитория из известного коммита (Sigstore; доверять каким-то ключам не нужно). Работает для архивов, APK и пакета для Android.

3. **Подпись Android.** APK должен быть подписан этим сертификатом (`apksigner verify --print-certs <apk>` или сведения о приложении в Android), SHA-256:

   `9F:FB:61:1C:DD:1B:3A:AE:63:30:3F:B7:12:0B:85:F7:67:0D:08:BD:B8:24:D6:7C:7C:85:37:09:88:0C:2A:7F`

4. **Соберите сами.** Сборки для Linux и приложение для Android воспроизводимы: каждый релиз собирается дважды на разных машинах и публикуется, только если байты совпали. Чтобы проверить сборку для Linux самому, нужны [zig](https://ziglang.org) и `cargo install cargo-zigbuild`:

   ```sh
   git clone https://github.com/prophecy1345/termchat && cd termchat
   git checkout v<версия> && git submodule update --init
   BUILD=zigbuild scripts/build-release.sh x86_64-unknown-linux-musl
   sha256sum target/x86_64-unknown-linux-musl/release/termchat target/x86_64-unknown-linux-musl/release/termchat-relay
   ```

   Хэши должны совпасть с `BINARIES.sha256` в архиве. Сборки для macOS и Windows пока не воспроизводимы: пересборка там даёт другие байты.

Зависимости проверяются `cargo deny` (лицензии, известные уязвимости) и `cargo vet` (аудиты Mozilla, Google и других; остальное перечислено в `supply-chain/` как ещё не проверенное).
