# syntax=docker/dockerfile:1.7
FROM rust:1.98-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY checks ./checks
RUN --mount=type=cache,id=b10x-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=b10x-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=llm-gateway-target,target=/src/target,sharing=locked \
    cargo build --release --locked -p b10x-llm-gateway-cli && \
    install -D /src/target/release/b10x-llm-gateway /out/b10x-llm-gateway

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f
ARG SOURCE_SHA=unknown
LABEL org.opencontainers.image.revision=$SOURCE_SHA
COPY --from=builder /out/b10x-llm-gateway /usr/local/bin/b10x-llm-gateway
USER nonroot:nonroot
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/b10x-llm-gateway"]
