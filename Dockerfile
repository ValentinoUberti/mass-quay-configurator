FROM rust:1.68.1 AS builder
COPY . .
RUN cargo build --release --offline

FROM registry.access.redhat.com/ubi8-micro
COPY --from=builder ./target/release/mqc ./target/release/mqc

ENTRYPOINT ["/target/release/mqc"]
