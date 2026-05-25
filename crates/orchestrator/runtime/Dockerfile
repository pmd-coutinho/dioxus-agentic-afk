# syntax=docker/dockerfile:1.6
#
# Runtime image for the Codex Sandbox. Carries codex, git, mise, and a
# minimal base; the entrypoint runs `mise install` against the bind-mounted
# Assignment Worktree's mise.toml so per-Project toolchains come from the
# worktree rather than baked into the image. See ADR-0041.
FROM debian:bookworm-slim

ARG MISE_VERSION=v2025.10.0
ARG PLAYWRIGHT_VERSION=1.60.0

ENV DEBIAN_FRONTEND=noninteractive \
    MISE_DATA_DIR=/cache/mise \
    MISE_CACHE_DIR=/cache/mise/cache \
    MISE_CONFIG_DIR=/cache/mise/config \
    RUSTUP_SELF_UPDATE=disable \
    PLAYWRIGHT_BROWSERS_PATH=/ms-playwright

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        git \
        bash \
        openssh-client \
        gnupg \
        xz-utils \
        nodejs \
        npm \
        ripgrep \
        libatomic1 \
        build-essential \
        pkg-config \
        libssl-dev \
        libglib2.0-0 \
        libnspr4 \
        libnss3 \
        libatk1.0-0 \
        libatk-bridge2.0-0 \
        libdbus-1-3 \
        libcups2 \
        libxcb1 \
        libxkbcommon0 \
        libasound2 \
        libgbm1 \
        libx11-6 \
        libxext6 \
        libcairo2 \
        libpango-1.0-0 \
        libxcomposite1 \
        libxdamage1 \
        libxfixes3 \
        libxrandr2 \
        libatspi2.0-0 \
 && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /ms-playwright \
 && chmod 0777 /ms-playwright \
 && PLAYWRIGHT_BROWSERS_PATH=/ms-playwright npx --yes "playwright@${PLAYWRIGHT_VERSION}" install chromium

# Docker initializes a fresh named volume from the image path it is mounted
# over. Keep the mise cache writable for containers that run as the host UID.
# The Codex home also needs to exist before file bind mounts land under
# /tmp/codex-home/.codex; otherwise Docker-created parent dirs may be
# root-owned and `mise trust` cannot write ~/.local/state.
RUN mkdir -p /cache/mise /tmp/codex-home/.codex \
 && chmod -R 0777 /cache/mise /tmp/codex-home

# Install mise into /usr/local/bin so it is on PATH for every UID the
# container is run with (the host's developer UID via --user).
RUN curl -fsSL https://mise.run | MISE_VERSION="${MISE_VERSION}" MISE_INSTALL_PATH=/usr/local/bin/mise sh

RUN printf '%s\n' 'export PATH="/cache/mise/shims:$PATH"' > /etc/profile.d/mise-shims.sh

# Install codex CLI from the pinned GitHub release; the binary ends up on
# PATH as `codex`.
ARG CODEX_VERSION=0.133.0
ARG TARGETARCH
RUN case "${TARGETARCH}" in \
        amd64) codex_arch="x86_64" ;; \
        arm64) codex_arch="aarch64" ;; \
        *) echo "unsupported Docker TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac \
 && codex_bin="codex-${codex_arch}-unknown-linux-musl" \
 && curl -fsSL "https://github.com/openai/codex/releases/download/rust-v${CODEX_VERSION}/${codex_bin}.tar.gz" \
      -o /tmp/codex.tar.gz \
 && tar -xzf /tmp/codex.tar.gz -C /usr/local/bin "${codex_bin}" \
 && mv "/usr/local/bin/${codex_bin}" /usr/local/bin/codex \
 && rm /tmp/codex.tar.gz \
 && chmod +x /usr/local/bin/codex

COPY entrypoint.sh /usr/local/bin/agentic-afk-entrypoint
RUN chmod +x /usr/local/bin/agentic-afk-entrypoint

WORKDIR /work
ENTRYPOINT ["/usr/local/bin/agentic-afk-entrypoint"]
