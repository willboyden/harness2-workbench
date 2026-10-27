# Runner image: the toolchain harness2 needs, and nothing else.
#
# Used two ways:
#   1. as the image for container-isolated jobs (`[isolation] mode = "container"`): the server
#      mounts the experiment, the harness2 checkout and the job's generated files into it;
#   2. as the base of the all-in-one Workbench image (deploy/docker/Dockerfile), where the whole
#      server runs contained and jobs run "host"-mode inside the container.
#
# Contents: Node 22 + npm (harness2 setup installs Bun and OpenCode through npm), uv with a
# managed CPython 3.12 (harness2's benchmarks require 3.12), git, curl, unzip, Pandoc and
# Poppler (LAB documents), and ripgrep (OpenCode's grep, glob and list tools: without `rg` on
# PATH OpenCode downloads it from GitHub, which a job behind the egress proxy cannot reach), and
# a `podman` stand-in that runs only LAB's document parser (deploy/docker/podman-standin.sh: LAB
# parses documents with `podman run`, which a container job cannot do).
# No harness2 code is baked in: the checkout is mounted, so the image stays generic and a
# harness2 bump never requires a rebuild.
#
# Every base is pinned by digest. Dependabot proposes Node updates (14-day cooldown); uv is
# bumped by hand, because its registry gives Dependabot no publication date to check the
# cooldown against (docs/SUPPLY-CHAIN.md, "Bumping uv by hand").
# Build: docker build -f deploy/docker/runner.Dockerfile -t harness2-workbench-runner:0.1.3 .

FROM ghcr.io/astral-sh/uv:0.12.24@sha256:3af4716e991d6956a41e573eab705d0ee08500cd829ed30293eb8472f372c65a AS uv

FROM docker.io/library/node:22-trixie-slim@sha256:8eb619e402eaede1748a3c8173bffab391592812e85136452f781741c7242ddf

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl git unzip pandoc poppler-utils ripgrep tini \
    && rm -rf /var/lib/apt/lists/*

COPY --from=uv /uv /uvx /usr/local/bin/
COPY --chmod=0755 deploy/docker/podman-standin.sh /usr/local/bin/podman
# uv's licence texts, which its image does not carry (the files of the pinned tag; update them
# with uv, docs/SUPPLY-CHAIN.md). Debian packages keep theirs in /usr/share/doc/*/copyright.
COPY deploy/docker/licenses/uv/ /usr/share/doc/uv/

# A managed CPython shared by every user of the image, outside any home directory.
ENV UV_PYTHON_INSTALL_DIR=/opt/uv/python \
    UV_PYTHON_PREFERENCE=only-managed \
    UV_NO_PROGRESS=1 \
    PYTHONDONTWRITEBYTECODE=1
RUN uv python install 3.12 \
    && chmod -R a+rX /opt/uv

# Who jobs run as is the server's choice (`--user`): its own uid:gid on a rootful runtime, or
# 0:0 on a rootless one, where container root IS the server's unprivileged host user (the only
# id that can write the bind-mounted experiment) and holds no capabilities (`--cap-drop ALL`).
# Without CAP_DAC_OVERRIDE that user cannot write /home/node, so HOME (uv, npm and Bun caches,
# OpenCode's state) is a sticky world-writable directory in the throwaway container layer.
RUN install -d -m 1777 /h2w/home
ENV HOME=/h2w/home

# Manual runs default to the node image's non-root user (uid 1000).
USER node
WORKDIR /h2w

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["bash", "-lc", "uv --version && node --version && git --version"]
