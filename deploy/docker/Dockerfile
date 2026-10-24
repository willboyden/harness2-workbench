# All-in-one Harness² Studio image: the server and the MCP server on top of the runner
# toolchain, so the whole Studio (and every harness2 job it starts in "host" isolation) runs
# inside one container. Pair it with deploy/compose.yaml, which puts it on an internal network
# whose only way out is the allowlisting egress proxy.
#
# Build from the repository root:
#   docker build -f deploy/docker/runner.Dockerfile -t harness2-studio-runner:0.1.0 .
#   docker build -f deploy/docker/Dockerfile -t harness2-studio:0.1.0 .

FROM docker.io/library/rust:1.99.1-slim-trixie@sha256:aab42391a986424e22d6a3ec14f37bc6dd3dacff4d457ca7cccd781e02297cc6 AS build
RUN rustup target add wasm32-unknown-unknown \
    && cargo install --locked wasm-bindgen-cli --version 0.2.129
WORKDIR /src
COPY . .
RUN cargo xtask web \
    && cargo build --release --locked -p h2s-daemon -p h2s-mcp

FROM harness2-studio-runner:0.1.0
COPY --from=build /src/target/release/h2studio /src/target/release/h2studio-mcp /usr/local/bin/
# The server and its host-mode jobs run as node, so they get node's private home back (the
# runner's shared HOME is for container jobs of any uid).
ENV H2S_DATA_DIR=/data \
    HOME=/home/node
USER node
WORKDIR /data
EXPOSE 8722
HEALTHCHECK --interval=30s --timeout=5s --retries=3 CMD ["curl", "-fsS", "http://127.0.0.1:8722/healthz"]
ENTRYPOINT ["/usr/bin/tini", "--", "h2studio"]
CMD ["serve", "--config", "/config/h2studio.toml"]
