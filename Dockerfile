FROM --platform=$BUILDPLATFORM rust:1.98-bookworm AS builder

ARG BUILDARCH
ARG TARGETARCH

RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-gnu > /rust-target ;; \
      arm64) echo aarch64-unknown-linux-gnu > /rust-target ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
  && rustup target add "$(cat /rust-target)" \
  && if [ "$TARGETARCH" != "$BUILDARCH" ]; then \
      dpkg --add-architecture "$TARGETARCH" \
      && apt-get update \
      && apt-get install -y --no-install-recommends \
        "crossbuild-essential-$TARGETARCH" "libssl-dev:$TARGETARCH" \
      && rm -rf /var/lib/apt/lists/*; \
    fi

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AARCH64_UNKNOWN_LINUX_GNU_OPENSSL_LIB_DIR=/usr/lib/aarch64-linux-gnu \
    AARCH64_UNKNOWN_LINUX_GNU_OPENSSL_INCLUDE_DIR=/usr/include \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    X86_64_UNKNOWN_LINUX_GNU_OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu \
    X86_64_UNKNOWN_LINUX_GNU_OPENSSL_INCLUDE_DIR=/usr/include

WORKDIR /src

COPY . .

ARG VERSION

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,id=datum-connect-target-$TARGETARCH \
    if [ -n "$VERSION" ]; then \
      sed -i "s/^version = \".*\"/version = \"${VERSION#v}\"/" cli/Cargo.toml lib/Cargo.toml \
      && cargo update --workspace; \
    fi \
    && target="$(cat /rust-target)" \
    && cargo build --locked --release -p datum-connect --target "$target" \
    && cp "target/$target/release/datum-connect" /usr/local/bin/datum-connect

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates libssl3 \
  && rm -rf /var/lib/apt/lists/* \
  && useradd -u 65532 -r -s /usr/sbin/nologin datum \
  && install -d -o 65532 -g 65532 /var/lib/datum-connect

COPY --from=builder /usr/local/bin/datum-connect /usr/local/bin/datum-connect

ENV DATUM_CONNECT_REPO=/var/lib/datum-connect

USER 65532:65532

ENTRYPOINT ["/usr/local/bin/datum-connect"]
CMD ["serve"]
