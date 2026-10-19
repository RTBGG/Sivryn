# syntax=docker/dockerfile:1
# Builds the sivryn-server image. Run from the repository root:
#   docker build -f deploy/Dockerfile .
#
# Both stages use Docker Hub images on the same Debian release, so the binary's
# glibc matches the runtime and no other registry needs to be reachable.

FROM rust:1-slim-trixie AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p sivryn-server \
    && cp target/release/sivryn-server /sivryn-server

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin sivryn
COPY --from=build /sivryn-server /usr/local/bin/sivryn-server
EXPOSE 8080
USER sivryn
ENTRYPOINT ["/usr/local/bin/sivryn-server"]
