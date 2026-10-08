# Tissue container image — #38 Deliverable B.
#
# Multi-stage: static musl build, then a minimal final stage containing
# only the tissue binary. The GUI assets are embedded in the binary
# (include_str!, PR #39), so no source tree or static assets are needed
# at runtime.
#
# Build:  docker build -f Dockerfile -t tissue:poc .
# Run:    docker run --rm --network host \
#           -v /path/to/myproject-issues:/issues tissue:poc serve --port 8080 --repo /issues
#
# Publishing is loopback-only by design: publish with -p 127.0.0.1:...
# Never bind the viewer to a public interface (AGENTS.md §5).

# ---------------------------------------------------------------------------
# Stage 1: build the static binary
# ---------------------------------------------------------------------------
FROM rust:1.85-slim AS build

RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /build
COPY src/ /build/src/
# Build with musl target; dependencies compile from the committed lockfile.
RUN cd src && cargo build --release --target x86_64-unknown-linux-musl
RUN test -x src/target/x86_64-unknown-linux-musl/release/tissue

# ---------------------------------------------------------------------------
# Stage 2: minimal final image — binary only
# ---------------------------------------------------------------------------
FROM scratch

COPY --from=build /build/src/target/x86_64-unknown-linux-musl/release/tissue /tissue

# The issue repo is provided by mounting a volume at /issues.
VOLUME /issues

ENTRYPOINT ["/tissue"]
CMD ["--help"]
