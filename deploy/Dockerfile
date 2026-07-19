# Build the cha-web server image.
#
#   docker build -f deploy/Dockerfile -t cha-web .
#
# Note the build context is the REPO ROOT, not deploy/ — the build needs
# cha-core, cha-web, and cha-gui/ui (the front end is embedded into the binary).
#
# The image deliberately ships NO dictionary; mount one at run time and point
# CHA_DICT_DIR at it. See deploy/README.md.

# ---- build ------------------------------------------------------------------
FROM rust:1-bookworm AS builder

# Never bake a word list into the image, regardless of whether words.txt happens
# to be in the build context. See cha-web/build.rs for why this is explicit
# rather than left to depend on which files got COPYed.
ENV CHA_NO_EMBED_WORDS=1

WORKDIR /src

# Copy manifests first so `cargo fetch` is cached independently of source edits.
# The workspace root Cargo.toml lists every member, so all of them must exist for
# the manifest to parse — hence the stub source files.
COPY Cargo.toml Cargo.lock ./
COPY cha-core/Cargo.toml cha-core/
COPY cha-web/Cargo.toml cha-web/
COPY cha-gui/src-tauri/Cargo.toml cha-gui/src-tauri/
RUN mkdir -p src cha-core/src cha-web/src cha-gui/src-tauri/src \
 && echo 'fn main(){}' > src/main.rs \
 && touch cha-core/src/lib.rs cha-gui/src-tauri/src/lib.rs \
 && echo 'fn main(){}' > cha-web/src/main.rs \
 && cargo fetch --locked

# Now the real sources. cha-gui/ui is required because cha-web embeds the front
# end with include_dir!; cha-gui/src-tauri is NOT built (we only build -p
# cha-web), but its manifest has to be present for the workspace to resolve.
COPY cha-core cha-core
COPY cha-web cha-web
COPY cha-gui/ui cha-gui/ui

# Bust the stub-source fingerprint so cargo rebuilds against the real code.
RUN touch cha-core/src/lib.rs cha-web/src/main.rs \
 && cargo build --release --locked -p cha-web \
 && strip target/release/cha-web

# ---- runtime ----------------------------------------------------------------
# debian-slim rather than a static/scratch image: cha-web needs only libc, but a
# shell in the image makes `docker exec` debugging possible for an operator who
# is not a Rust developer. The size difference is a few tens of MB.
FROM debian:bookworm-slim

RUN useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin cha

COPY --from=builder /src/target/release/cha-web /usr/local/bin/cha-web

# 0.0.0.0 is correct *inside a container*: the network namespace, not this
# setting, is what decides whether anything outside can reach the port. Binding
# loopback here would make the server unreachable even from the host. The
# server's own default stays 127.0.0.1 for people running it directly.
ENV CHA_BIND=0.0.0.0 \
    CHA_PORT=8080 \
    CHA_DICT_DIR=/dictionaries

# Where to mount word lists. Created so the container starts with a valid (if
# empty) path; an empty directory still exits with a clear message.
RUN mkdir -p /dictionaries && chown cha:cha /dictionaries
VOLUME ["/dictionaries"]

USER cha
EXPOSE 8080

# cha-web probes itself, so the runtime image needs no curl or wget.
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ["/usr/local/bin/cha-web", "--health-check"]

# Exec form (not shell form) so cha-web is PID 1 and receives SIGTERM directly;
# it handles that for a clean, immediate `docker stop`.
ENTRYPOINT ["/usr/local/bin/cha-web"]
