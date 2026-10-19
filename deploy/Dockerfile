# syntax=docker/dockerfile:1
# Builds the sivryn-server image. Run from the repository root:
#   docker build -f deploy/Dockerfile .

FROM rust:1-slim AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p sivryn-server \
    && cp target/release/sivryn-server /sivryn-server

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /sivryn-server /usr/local/bin/sivryn-server
EXPOSE 8080
USER nonroot
ENTRYPOINT ["/usr/local/bin/sivryn-server"]
