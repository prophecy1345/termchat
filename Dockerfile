# Relay image: static musl binary on an empty base (docs/self-host.md).
FROM rust:1.99-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p termchat-relay && mkdir /data

FROM scratch
COPY --from=build /src/target/release/termchat-relay /termchat-relay
COPY --from=build --chown=65534:65534 /data /data
USER 65534:65534
WORKDIR /data
EXPOSE 8080 8081
ENTRYPOINT ["/termchat-relay", "--listen", "0.0.0.0:8080"]
