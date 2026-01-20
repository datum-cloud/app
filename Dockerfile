FROM rust:1.88-bookworm AS builder

WORKDIR /app

COPY . .

RUN cargo build -p datum-connect --release

FROM debian:bookworm-slim

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/* \
  && useradd -u 65532 -r -s /usr/sbin/nologin datum

COPY --from=builder /app/target/release/datum-connect /usr/local/bin/datum-connect

USER 65532:65532

ENTRYPOINT ["/usr/local/bin/datum-connect"]
CMD ["gateway"]
