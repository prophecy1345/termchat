# Relay image: static musl binary on an empty base (docs/self-host.md).
# The base image is pinned by digest (the multi-arch index), so a build pulls the
# same toolchain the maintainers vetted. Update: docker buildx imagetools inspect rust:<tag>-alpine
FROM rust:1.99-alpine@sha256:557a94c82b485a14b569120e00867267108c734efe33f10cc31ca0cd5dd1eca9 AS build
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
