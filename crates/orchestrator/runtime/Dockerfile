# syntax=docker/dockerfile:1.6
#
# Runtime image for the Codex Sandbox. Carries codex, git, mise, and a
# minimal base; the entrypoint runs `mise install` against the bind-mounted
# Assignment Worktree's mise.toml so per-Project toolchains come from the
# worktree rather than baked into the image. See ADR-0041.
FROM debian:bookworm-slim

ARG MISE_VERSION=v2025.10.0

ENV DEBIAN_FRONTEND=noninteractive \
    MISE_DATA_DIR=/cache/mise \
    MISE_CACHE_DIR=/cache/mise/cache \
    MISE_CONFIG_DIR=/cache/mise/config

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        git \
        bash \
        openssh-client \
        gnupg \
        xz-utils \
 && rm -rf /var/lib/apt/lists/*

# Install mise into /usr/local/bin so it is on PATH for every UID the
# container is run with (the host's developer UID via --user).
RUN curl -fsSL https://mise.run | MISE_VERSION="${MISE_VERSION}" MISE_INSTALL_PATH=/usr/local/bin/mise sh

# Install codex CLI. Pulled from the published npm package via a pinned
# version; the binary ends up on PATH as `codex`.
ARG CODEX_VERSION=0.21.0
RUN curl -fsSL "https://github.com/openai/codex/releases/download/v${CODEX_VERSION}/codex-x86_64-unknown-linux-musl.tar.gz" \
      -o /tmp/codex.tar.gz \
 && tar -xzf /tmp/codex.tar.gz -C /usr/local/bin codex \
 && rm /tmp/codex.tar.gz \
 && chmod +x /usr/local/bin/codex

COPY entrypoint.sh /usr/local/bin/agentic-afk-entrypoint
RUN chmod +x /usr/local/bin/agentic-afk-entrypoint

WORKDIR /work
ENTRYPOINT ["/usr/local/bin/agentic-afk-entrypoint"]
