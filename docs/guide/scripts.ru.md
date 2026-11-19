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
| `daemon` | держит личность открытой, и команды выше работают без пароля и отвечают сразу (Linux, macOS; см. ниже) |

Каждая команда принимает также `--password-file PATH` или `--password-stdin` и `--timeout SECS` (по умолчанию 60). Папка состояния — `TERMCHAT_HOME` или `~/.termchat`.

- `CONTACT` — имя, которое показывает приложение, или начало отпечатка. Если подходит больше одного контакта, ничего не отправляется.
- **Пароль никогда не берётся из переменных окружения**: их наследует каждый дочерний процесс. Используйте файл, который принадлежит вам и имеет права `600`, stdin или клавиатуру. Без них termchat спрашивает пароль с клавиатуры, а у скрипта её нет: он завершится с «no terminal to type the password».
- Отправки «всем» нет: одна команда — один контакт.
- Запросы контакта принимает владелец в интерфейсе приложения. У CLI и daemon команды принятия нет.
- Текст собеседника очищается от управляющих символов, прежде чем попасть в ваш терминал или лог. Файлы сохраняются только в указанную папку и никогда не открываются.

Коды возврата: 0 готово, 1 пароль или состояние, 2 аргументы, 3 контакт не найден или найдено несколько, 4 relay не принял вовремя (уйдёт со следующей командой), 5 этому контакту писать нельзя. Команда выходит, только когда relay принял всё, что она отправила, в пределах `--timeout`.

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

Одна личность выполняет одну команду за раз: вторая в тот же момент завершится с кодом 1 и скажет, что личность открыта в другом месте. Служба снимает это ограничение.

## Служба для бота, который отвечает

Каждая команда выше заново расшифровывает личность и подключается к relay: через Tor это 10–30 секунд на команду. `termchat daemon` делает это один раз и остаётся (ADR-029). Пока она работает, `send`, `send-file`, `recv`, `contacts`, `invite` и `add` для того же `TERMCHAT_HOME` идут через неё: без пароля, с ответом за секунду и по нескольку сразу. Если службы нет, всё работает как раньше.

```ini
# /etc/systemd/system/termchat-daemon.service
[Unit]
Description=termchat daemon
After=network-online.target
Wants=network-online.target

[Service]
User=termchat-bot
Environment=TERMCHAT_HOME=/var/lib/termchat-bot
ExecStart=/usr/local/bin/termchat daemon --password-file /etc/termchat-bot/pw
Restart=on-failure
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=/var/lib/termchat-bot
PrivateTmp=yes

[Install]
WantedBy=multi-user.target
```

Бот, который отвечает на «status» и «disk» с телефона (`jq` разбирает JSON):

```sh
export TERMCHAT_HOME=/var/lib/termchat-bot
termchat recv --from alice --follow --json | while read -r line; do
  case "$(printf '%s' "$line" | jq -r .text)" in
    status) termchat send alice "$(uptime)" ;;
    disk)   termchat send alice "$(df -h / | tail -1)" ;;
  esac
done
```

- Служба слушает `daemon.sock` в папке состояния с правами `600` и пускает только своего пользователя. В сети ничего не слушает.
- **Пока она работает, пароль и ключи у неё в памяти, и любой процесс этого пользователя может писать через неё.** Давайте ей серверную личность под отдельным пользователем, никогда не основную.
- Отправляемый документ читает команда, а не служба; папка для `recv --save-dir` должна существовать заранее.
- Останавливается по SIGTERM или Ctrl+C: до 5 с ждёт, пока relay примет только что отправленное, потом сохраняет. На Windows службы нет.

## Получить сообщение, когда сервер замолчит

Заведите отдельные личности на двух хостах по шагам 1–3 из раздела «Как завести сервер». Назовём наблюдаемый сервер `backup-box`, сторожа `watcher`, а получателя тревог `alice`. Используйте `bot` из того же раздела на каждом хосте.

1. На хосте `watcher` выполните `bot invite` и скопируйте всю строку `termchat:…`. На `backup-box` выполните `bot add 'termchat:…'`, подставив эту строку.
2. Если daemon уже работает, остановите его на каждом хосте на время настройки. Для unit-файла из примера подойдёт `sudo systemctl stop termchat-daemon`. Откройте `termchat` без подкоманды от того же пользователя и с тем же `TERMCHAT_HOME`, что у службы.

   ```sh
   sudo -u termchat-bot env TERMCHAT_HOME=/var/lib/termchat-bot termchat
   ```

   Введите пароль служебной личности на каждом хосте.

3. В интерфейсе `watcher` выберите запрос от `backup-box` и нажмите **Ctrl+Y**. После принятия обе личности появятся друг у друга в контактах. На каждом хосте выберите этот контакт, откройте номер безопасности через **Ctrl+F**, сравните номера и при совпадении нажмите **v** на обоих хостах.
4. На `watcher` нажмите **Ctrl+A**, вставьте приглашение владельца и нажмите **Enter**. Владелец принимает запрос в своём приложении. Сравните номер безопасности с владельцем и отметьте его контакт как проверенный на `watcher`. [Как принимать запросы и сверять номера безопасности](contacts.ru.md)

Закройте оба терминальных интерфейса и запустите `termchat daemon` для каждой личности. Команды наблюдения запускайте от того же пользователя, с тем же `TERMCHAT_HOME` и с именами из `bot contacts` на соответствующем хосте.

```sh
# На наблюдаемом сервере, с TERMCHAT_HOME серверной личности
termchat watch send watcher --every 300

# На втором хосте, с TERMCHAT_HOME личности сторожа
termchat watch monitor backup-box --notify alice --after 900
```

В примере сервер шлёт пульс раз в пять минут, а сторож пишет после 15 минут тишины. Указывайте интервалы в секундах и оставляйте запас на переподключение. Обе команды работают на переднем плане. Запускайте их через менеджер служб, чтобы они перезапускались после остановки daemon или разрыва сокета. Пульсы скрыты из истории и `recv`, тревога приходит обычным личным сообщением.

Держите сторожа включённым и синхронизируйте часы. Сбой relay может вызвать тревогу или помешать её доставке. Daemon пишет в локальный журнал, когда ставит тревогу в очередь и когда не может подтвердить доставку. Свежий пульс позволяет сторожу сообщить о следующем сбое.

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
