# Свой relay

English: [self-host.md](self-host.md).

Relay пересылает зашифрованные конверты. Он никогда не видит содержимое
сообщений, ники и списки контактов. Что он всё-таки видит, описано в
[threat-model.ru.md](threat-model.ru.md).

Нужен небольшой Linux-сервер с доменом (для HTTPS) и, по желанию, Tor (чтобы
дать ещё и onion-адрес).

## Что хранит relay

Всё в папке данных, права `0600` (ADR-005):

| Файл | Содержимое | Сколько хранится |
|---|---|---|
| `termchat-relay.db` (+ `-wal`, `-shm`) | публичные ключи, prekeys, недоставленный шифртекст с отправителем, получателем и временем, баны | конверты до доставки или 14 дней; аккаунты до 90 дней без подключений |
| `termchat-reports.log` | жалобы, которые пользователи сами решили вам отправить (ADR-006): время, ключ, раскрытый текст | пока вы не удалите |

Больше ничего: ни IP-адресов, ни журнала доступа, ни истории сообщений.

## Вариант А: Docker + Caddy (проще всего)

```sh
git clone https://github.com/prophecy1345/termchat && cd termchat/deploy
# впишите свой домен в Caddyfile и docker-compose.yml (relay.example.org)
docker compose up -d
```

Caddy получает сертификат Let's Encrypt и пересылает WebSocket-трафик relay,
который слушает только внутри сети Docker. Caddyfile намеренно выбрасывает
журнал доступа. Пользователи при первом запуске вводят
`wss://relay.example.org/`.

## Вариант Б: бинарник + systemd

Скачайте релиз для своего сервера (`termchat-…-x86_64-unknown-linux-musl.tar.gz`),
проверьте его (см. README), затем:

```sh
sudo install -m 755 termchat-relay /usr/local/bin/
```

`/etc/systemd/system/termchat-relay.service`:

```ini
[Unit]
Description=termchat.app relay
After=network-online.target

[Service]
ExecStart=/usr/local/bin/termchat-relay --host relay.example.org --trust-proxy --listen 127.0.0.1:8080
DynamicUser=yes
StateDirectory=termchat-relay
WorkingDirectory=/var/lib/termchat-relay
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl enable --now termchat-relay
```

Поставьте перед ним Caddy (или nginx), который завершает TLS и проксирует на
`127.0.0.1:8080`:

```
relay.example.org {
    reverse_proxy 127.0.0.1:8080
    log {
        output discard
    }
}
```

Флаг `--trust-proxy` нужен, только если впереди стоит прокси, который ставит
`X-Forwarded-For`. Иначе любой сможет подделать свой IP и обойти лимиты по IP.

## Onion-адрес (Tor)

Поставьте Tor из пакетов дистрибутива (`apt install tor`; в пакете
Debian/Ubuntu есть защита proof-of-work). Добавьте в `/etc/tor/torrc`:

```
HiddenServiceDir /var/lib/tor/termchat/
HiddenServicePort 80 127.0.0.1:8081
HiddenServicePoWDefensesEnabled 1
```

В сборке Homebrew и некоторых других модуля proof-of-work нет (нужна сборка с
`--enable-gpl`); там уберите последнюю строку, иначе tor не запустится.

```sh
sudo systemctl restart tor
sudo cat /var/lib/tor/termchat/hostname   # xxxx.onion
```

Затем запустите relay с обоими именами и onion-портом:

```
termchat-relay --host relay.example.org --host xxxx.onion --trust-proxy \
  --listen 127.0.0.1:8080 --onion-listen 127.0.0.1:8081
```

Пользователи вводят `ws://xxxx.onion/` как onion-адрес и отвечают «y» на «Use
Tor?». У подключений через onion-порт общий бюджет (ADR-004), поэтому лимиты по
IP не отрезают пользователей Tor.

## Эксплуатация

- **Резервные копии.** Потеря базы — это потеря недоставленных сообщений и
  prekeys (клиенты опубликуют ключи заново при следующем подключении). Копия:
  `sqlite3 termchat-relay.db ".backup backup.db"`. В копии те же метаданные,
  защищайте её так же.
- **Жалобы.** Читайте `termchat-reports.log`, принимайте меры и удаляйте
  разобранные строки. Три жалобы от разных людей, которым этот ключ
  действительно писал, банят его автоматически. Баны лежат в таблице
  `accounts` (`banned = 1`).
- **Шифрование диска.** Включите полное шифрование диска на сервере: удалённые
  строки могут оставаться на SSD до TRIM.
- **Обновление.** Остановить, заменить бинарник, запустить. Версия схемы
  хранится в базе (`PRAGMA user_version`), чтобы следующие релизы могли её
  мигрировать; версия 2 добавляет endpoint UnifiedPush (ADR-012) и мигрирует при запуске. Клиенты сравнивают свою версию с
  той, что сообщает ваш relay, и говорят пользователю, если она новее
  (ADR-010), поэтому обновляйте relay: старый relay о новых релизах молчит.
- **Закрыть навсегда.** `systemctl stop termchat-relay`, затем удалить папку
  данных. С Docker: `docker compose down -v`.
