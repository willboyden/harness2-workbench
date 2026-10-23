# Runner image: the toolchain harness2 needs, and nothing else.
#
# Used two ways:
#   1. as the image for container-isolated jobs (`[isolation] mode = "container"`): the server
#      mounts the experiment, the harness2 checkout and the job's generated files into it;
#   2. as the base of the all-in-one Studio image (deploy/docker/Dockerfile), where the whole
#      server runs contained and jobs run "host"-mode inside the container.
#
# Contents: Node 22 + npm (harness2 setup installs Bun and OpenCode through npm), uv with a
# managed CPython 3.12 (harness2's benchmarks require 3.12), git, curl, unzip, Pandoc and
# Poppler (LAB documents). No harness2 code is baked in: the checkout is mounted, so the image
# stays generic and a harness2 bump never requires a rebuild.
#
# Every base is pinned by digest; Dependabot proposes updates (14-day cooldown).
# Build: docker build -f deploy/docker/runner.Dockerfile -t harness2-studio-runner:0.1.0 .

FROM ghcr.io/astral-sh/uv:0.12.24@sha256:3af4716e991d6956a41e573eab705d0ee08500cd829ed30293eb8472f372c65a AS uv

FROM docker.io/library/node:22-trixie-slim@sha256:8eb619e402eaede1748a3c8173bffab391592812e85136452f781741c7242ddf

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl git unzip pandoc poppler-utils tini \
    && rm -rf /var/lib/apt/lists/*

COPY --from=uv /uv /uvx /usr/local/bin/

# A managed CPython shared by every user of the image, outside any home directory.
ENV UV_PYTHON_INSTALL_DIR=/opt/uv/python \
    UV_PYTHON_PREFERENCE=only-managed \
    UV_NO_PROGRESS=1 \
    PYTHONDONTWRITEBYTECODE=1
RUN uv python install 3.12 \
    && chmod -R a+rX /opt/uv

# The node image's non-root user (uid 1000). Jobs never run as root.
USER node
ENV HOME=/home/node
WORKDIR /h2s

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["bash", "-lc", "uv --version && node --version && git --version"]
