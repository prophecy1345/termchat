# termchat command-line image (PLAN D3.5): `termchat send`, `run`, `ask`,
# `daemon`, `hook` as a sidecar in compose or a Kubernetes CronJob.
# Build from the repository root:
#   docker build -f deploy/cli.Dockerfile -t termchat .
# The identity lives in /data: mount a volume there, never bake it in.
# Same pinned toolchain as the relay image (Dockerfile).
FROM rust:1.99-alpine@sha256:557a94c82b485a14b569120e00867267108c734efe33f10cc31ca0cd5dd1eca9 AS build
RUN apk add --no-cache musl-dev ca-certificates
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p termchat && mkdir /data

FROM scratch
COPY --from=build /src/target/release/termchat /termchat
# wss:// needs root certificates; rustls reads this file.
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=build --chown=65534:65534 /data /data
ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt \
    TERMCHAT_HOME=/data
USER 65534:65534
WORKDIR /data
ENTRYPOINT ["/termchat"]
