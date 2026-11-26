---
format: aep.planning-md/3
id: story:musl-release-archive
kind: story
status: active
title: A static musl archive ships beside the gnu one in every release
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T07:06:52Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-26T07:06:52Z", actor: "human:operator", revision: 3}
---
## Outcome

Each llm-gateway release carries a static `x86_64-unknown-linux-musl` archive beside the gnu one,
which runs on any x86_64 Linux whatever its C library, and consumers pin that archive.

## Why

The owner approved on 2026-11-25 about 23:1x CET, for every beyond10x Rust tool: static musl Linux
releases, mimalloc where allocation-heavy, consumers pin the musl archives. A build node in use runs
glibc 2.34; gnu release binaries built against a newer glibc warn or fail there.

## Acceptance

1. The release workflow builds `b10x-llm-gateway` for `x86_64-unknown-linux-musl` and publishes
   its archive and checksum beside the gnu archive, named the same way with the musl triple.
2. A CI check on every release build, and on pull requests that change the workflow or the
   manifest: the binary has no dynamic section (no interpreter, `file` or `readelf -d` says
   static), and `--version` runs in a plain `alpine` container and prints the release version.
3. mimalloc as the global allocator of the musl build only if measured not slower: the relay's
   request throughput and the translated path's answer time, musl with and without mimalloc and the
   gnu build, on a stub upstream, reported in the pull request; kept only when the musl build with
   it is not slower than without.
4. The release guide and the install guide name the musl archive as the one to pin on Linux.
5. The container image is unchanged unless the pull request says why.
