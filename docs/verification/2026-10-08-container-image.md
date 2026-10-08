# Container image build, 2026-10-08

One local build of the root `Dockerfile` (row D1), at commit `b03382c` on `impl/container-image`.
Building the image is not part of `task check`; `crates/llm-gateway-cli/tests/dockerfile.rs`
checks the file's five D1 facts on every gate.

## Tools

| Tool | Version |
| --- | --- |
| `docker` | Docker version 29.7.2, build a7dcaa6fdb |
| `docker buildx` | github.com/docker/buildx 0.36.1 1d8dde89b8aba914e05e45366770736fea1fd690 |

## Build

```bash
docker build --build-arg SOURCE_SHA=b03382c -t llm-gateway:w03-local .
```

Exit status `0`. The builder stage compiled the workspace's `b10x-llm-gateway-cli` package
(`Finished release profile [optimized] target(s) in 45.24s`) from
`rust:1.98-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e`
onto `gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f`.

## The built image

```bash
docker image inspect llm-gateway:w03-local --format 'user={{.Config.User}} ports={{json .Config.ExposedPorts}} entrypoint={{json .Config.Entrypoint}} revision={{index .Config.Labels "org.opencontainers.image.revision"}} size={{.Size}}'
```

```text
user=nonroot:nonroot ports={"8080/tcp":{}} entrypoint=["/usr/local/bin/b10x-llm-gateway"] revision=b03382c size=25662617
```

`docker run --rm --network none llm-gateway:w03-local --help` printed the binary's usage
(`Usage: b10x-llm-gateway --config <CONFIG>`) and exited `0`.

The image was not pushed or tagged for a registry, and was deleted afterwards with
`docker image rm llm-gateway:w03-local` (exit `0`).
