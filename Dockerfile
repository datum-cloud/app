FROM rust:1.98-bookworm AS builder

WORKDIR /src

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --locked --release -p datum-connect \
    && cp target/release/datum-connect /usr/local/bin/datum-connect

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
