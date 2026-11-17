---
title: "termchat.app для скриптов и серверов: send, recv, cron, systemd"
description: "Отправляйте сообщения со сквозным шифрованием из shell-скрипта, задачи cron, CI или с сервера себе на телефон командой termchat send и читайте ответы через termchat recv, без бота с аккаунтом."
---

# Скрипты и серверы

English version: [scripts.md](scripts.md).

`termchat` без аргументов открывает экран чата. С командой он делает одно дело без экрана и завершается, поэтому скрипт, задача cron или сервер могут прислать вам сообщение со сквозным шифрованием или прочитать ваш ответ: «бэкап не прошёл» на телефоне, без Telegram-бота и без третьей стороны, которая это читает.

## Команды

| Команда | Что делает |
|---|---|
| `init --nickname NAME [--relay URL] [--onion URL] [--tor]` | создаёт личность |
| `invite` | печатает одноразовое приглашение |
| `add INVITE` | просит контакт; он принимает в своём приложении |
| `contacts` | имя, отпечаток и состояние, по строке на контакт |
| `send CONTACT [TEXT]` | отправляет TEXT или stdin, если TEXT не указан; ждёт, пока сообщение окажется на relay |
| `send-file CONTACT PATH` | документ до 1 МиБ |
| `recv [--from C] [--wait S] [--follow] [--json] [--save-dir DIR]` | печатает новые сообщения и изменения, каждое один раз; файлы сохраняются только в `--save-dir` |

Каждая команда принимает также `--password-file PATH` или `--password-stdin` и `--timeout SECS` (по умолчанию 60). Папка состояния — `TERMCHAT_HOME` или `~/.termchat`.

- `CONTACT` — имя, которое показывает приложение, или начало отпечатка. Если подходит больше одного контакта, ничего не отправляется.
- **Пароль никогда не берётся из переменных окружения**: их наследует каждый дочерний процесс. Используйте файл, который принадлежит вам и имеет права `600`, stdin или клавиатуру. Без них termchat спрашивает пароль с клавиатуры, а у скрипта её нет: он завершится с «no terminal to type the password».
- Отправки «всем» нет: одна команда — один контакт.
- Серверная личность не может принимать запросы: связь заводите вы из своего приложения, поэтому она никогда не впустит незнакомца.
- Текст собеседника очищается от управляющих символов, прежде чем попасть в ваш терминал или лог. Файлы сохраняются только в указанную папку и никогда не открываются.

Коды возврата: 0 готово, 1 пароль или состояние, 2 аргументы, 3 контакт не найден или найдено несколько, 4 relay не принял вовремя (уйдёт со следующей командой), 5 этому контакту писать нельзя.

## Как завести сервер

Шаги для Linux с systemd, команды от root. Дайте серверу **отдельную личность**, никогда не основную: тот, кто получит root на этом сервере, прочитает всё, что эта личность отправляет и получает, и сможет писать от её имени.

```sh
# 1. termchat для всех пользователей сервера
curl -fsSL https://raw.githubusercontent.com/prophecy1345/termchat/main/scripts/install.sh | TERMCHAT_INSTALL_DIR=/usr/local/bin sh

# 2. системный пользователь, которому принадлежит только эта личность, её папка и пароль, читаемый только им
useradd --system --home-dir /var/lib/termchat-bot --shell /usr/sbin/nologin termchat-bot
install -d -o termchat-bot -g termchat-bot -m 700 /var/lib/termchat-bot /etc/termchat-bot
head -c 30 /dev/urandom | base64 | sudo -u termchat-bot sh -c 'umask 077; cat > /etc/termchat-bot/pw'

# короткое имя для следующих шагов
bot() { sudo -u termchat-bot env TERMCHAT_HOME=/var/lib/termchat-bot termchat "$@" --password-file /etc/termchat-bot/pw; }

# 3. личность; ник — то, что вы увидите в приложении
bot init --nickname backup-box

# 4. ваше приглашение: Ctrl+O в терминале, Invite в приложении для компьютера, Your invite на Android
bot add 'termchat:...'
```

5. Примите запрос от `backup-box` в своём приложении.
6. Посмотрите, под каким именем сервер знает вас, и отправьте пробное сообщение:

```sh
bot contacts
bot send alice "привет от $(hostname)"
```

## Из systemd или cron

Служба или задача cron не видит того, что вы задали в своей оболочке: передайте ей `TERMCHAT_HOME` и файл пароля сами.

```ini
# /etc/systemd/system/disk-report.service
[Service]
Type=oneshot
User=termchat-bot
Environment=TERMCHAT_HOME=/var/lib/termchat-bot
ExecStart=/bin/sh -c 'df -h / | /usr/local/bin/termchat send alice --password-file /etc/termchat-bot/pw'
```

Для расписания добавьте `.timer`. С cron — `crontab -u termchat-bot -e`:

```
0 9 * * * TERMCHAT_HOME=/var/lib/termchat-bot /usr/local/bin/termchat send alice "доброе утро" --password-file /etc/termchat-bot/pw
```

`LoadCredential=` из systemd не подходит: копия, которую он делает, принадлежит root, а termchat принимает только файл пароля своего пользователя.

Одна личность выполняет одну команду за раз: вторая в тот же момент завершится с кодом 1 и скажет, что личность открыта в другом месте.

## Ещё примеры

```sh
# конец долгой команды, на ноутбуке (опять отдельная личность)
make release; termchat send alice "сборка релиза: код $?" --password-file ~/.termchat-bot.pw

# текст из пайпа
df -h / | termchat send alice --password-file /etc/termchat-bot/pw

# ждать ответа до 10 минут, строками JSON
termchat recv --from alice --wait 600 --json --password-file /etc/termchat-bot/pw
```

## Что печатает recv

Обычный вывод: `alice: текст`, `alice edited: текст`, `alice deleted a message`, `alice reacted like to: текст`.

С `--json` — один объект на строку:

```json
{"at":1794913363,"fingerprint":"78125 66938 20371 20023 95579 06120","from":"alice·78125","id":"17037012887881307594","kind":"react","mine":true,"reaction":"love"}
{"at":1794913393,"fingerprint":"78125 66938 20371 20023 95579 06120","from":"alice·78125","id":"5136533931879411279","kind":"message","reply_to":{"id":"5235206626092912747","mine":true},"text":"which disk? / or /var?"}
```

- `kind`: `message`, `edit`, `delete` или `react`.
- `id`: сообщение, строкой (числа больше 2^53 читатели JSON искажают). Для `react` поле `mine` говорит, ваше ли это сообщение; `reaction` — одно из `like`, `love`, `laugh`, `wow`, `sad`, `thanks`, `dislike` или `null`, если реакцию убрали.
- `reply_to`: на что отвечает сообщение; `mine` там говорит, было ли оно вашим.
- `at`: когда пришло само сообщение.

Сообщение, исправленное до того, как `recv` его забрал, приходит один раз, уже в новом виде; удалённое до этого не приходит вовсе.
