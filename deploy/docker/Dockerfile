# All-in-one Harness² Workbench image: the server and the MCP server on top of the runner
# toolchain, so the whole Workbench (and every harness2 job it starts in "host" isolation) runs
# inside one container. Pair it with deploy/compose.yaml, which puts it on an internal network
# whose only way out is the allowlisting egress proxy.
#
# Build from the repository root:
#   docker build -f deploy/docker/runner.Dockerfile -t harness2-workbench-runner:0.1.1 .
#   docker build -f deploy/docker/Dockerfile -t harness2-workbench:0.1.1 .
# `--build-arg RUNNER_IMAGE=<name:tag>` builds on another runner image.

# Built locally from runner.Dockerfile, never pulled. Naming it through an argument also keeps
# Dependabot from looking it up on Docker Hub.
ARG RUNNER_IMAGE=harness2-workbench-runner:0.1.1

FROM docker.io/library/rust:1.99.1-slim-trixie@sha256:aab42391a986424e22d6a3ec14f37bc6dd3dacff4d457ca7cccd781e02297cc6 AS build
RUN rustup target add wasm32-unknown-unknown \
    && cargo install --locked wasm-bindgen-cli --version 0.2.129 \
    && cargo install --locked cargo-about --version 0.9.2 --features cli
WORKDIR /src
COPY . .
# BuildKit cache mounts keep crates.io downloads and the build directory between image builds;
# the binaries and the licence file are copied out because a cache mount is not part of the image.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo xtask web \
    && cargo build --release --locked -p h2w-daemon -p h2w-mcp \
    && cargo xtask licenses \
    && install -D -m 0755 -t /out target/release/h2workbench target/release/h2workbench-mcp \
    && install -D -m 0644 -t /out dist/THIRD-PARTY-LICENSES.md

FROM ${RUNNER_IMAGE}
COPY --from=build /out/h2workbench /out/h2workbench-mcp /usr/local/bin/
COPY --from=build /out/THIRD-PARTY-LICENSES.md /usr/share/doc/harness2-workbench/
COPY LICENSE NOTICE /usr/share/doc/harness2-workbench/
# The server and its host-mode jobs run as node, so they get node's private home back (the
# runner's shared HOME is for container jobs of any uid). H2W_CONFIG lets every subcommand
# (`init`, `token create`, `doctor`, ...) find the mounted configuration without --config.
ENV H2W_DATA_DIR=/data \
    H2W_CONFIG=/config/h2workbench.toml \
    HOME=/home/node
USER node
WORKDIR /data
EXPOSE 8722
HEALTHCHECK --interval=30s --timeout=5s --retries=3 CMD ["curl", "-fsS", "http://127.0.0.1:8722/healthz"]
ENTRYPOINT ["/usr/bin/tini", "--", "h2workbench"]
CMD ["serve"]
