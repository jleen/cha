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
# `--platform=$BUILDPLATFORM` pins the builder to the machine doing the building
# and cross-compiles from there, instead of emulating the target under QEMU.
# For Rust that is the difference between minutes and tens of minutes: rustc is
# exactly the kind of long-running CPU-bound workload emulation is worst at.
#
# This is cheap here only because cha-web's dependency tree is pure Rust — no
# -sys crates, no `cc`, no build scripts other than cha-web's own (which runs on
# the host anyway). So cross-compiling needs a linker and nothing else. Verify
# that assumption still holds before adding a dependency with C in it:
#
#     cargo tree -p cha-web -e build | grep -iE '\bcc v|cmake|bindgen'
#
# If that ever finds something, this stage needs a full cross C toolchain and
# CC_<triple>/AR_<triple> wiring, and going back to QEMU may be the better trade.
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS builder

# Set by buildx for each requested platform.
ARG TARGETARCH

# Never bake a word list into the image, regardless of whether words.txt happens
# to be in the build context. See cha-web/build.rs for why this is explicit
# rather than left to depend on which files got COPYed.
ENV CHA_NO_EMBED_WORDS=1

# Consulted only when the target is arm64; harmless otherwise. Cargo reads this
# per-target env var to pick the linker.
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc

# Map Docker's arch name to a Rust target triple, and install the cross linker
# when one is needed. The triple is stashed in a file so later layers can read it
# without recomputing the mapping.
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) triple=x86_64-unknown-linux-gnu ;; \
      arm64) triple=aarch64-unknown-linux-gnu; \
             apt-get update; \
             apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu; \
             rm -rf /var/lib/apt/lists/* ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    echo "$triple" > /rust-target; \
    rustup target add "$triple"

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

# Compile the dependency graph against the stubs. cargo builds everything
# cha-web *declares*, not just what the stub source happens to reference, so this
# warms the expensive part of the build in a layer that only changes when a
# manifest or the lockfile does. Source edits below then recompile just cha-core
# and cha-web.
RUN cargo build --release --locked -p cha-web --target "$(cat /rust-target)"

# Now the real sources. cha-gui/ui is required because cha-web embeds the front
# end with include_dir!; cha-gui/src-tauri is NOT built (we only build -p
# cha-web), but its manifest has to be present for the workspace to resolve.
COPY cha-core cha-core
COPY cha-web cha-web
COPY cha-gui/ui cha-gui/ui

# Bust the stub-source fingerprint so cargo rebuilds against the real code, then
# park the binary at a fixed path — the target directory is triple-dependent and
# the runtime stage shouldn't have to know which.
RUN set -eux; \
    triple="$(cat /rust-target)"; \
    touch cha-core/src/lib.rs cha-web/src/main.rs; \
    cargo build --release --locked -p cha-web --target "$triple"; \
    mkdir -p /out; \
    cp "target/$triple/release/cha-web" /out/cha-web; \
    case "$TARGETARCH" in \
      arm64) aarch64-linux-gnu-strip /out/cha-web ;; \
      *)     strip /out/cha-web ;; \
    esac

# ---- runtime ----------------------------------------------------------------
# debian-slim rather than a static/scratch image: cha-web needs only libc, but a
# shell in the image makes `docker exec` debugging possible for an operator who
# is not a Rust developer. The size difference is a few tens of MB.
FROM debian:bookworm-slim

RUN useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin cha

COPY --from=builder /out/cha-web /usr/local/bin/cha-web

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
