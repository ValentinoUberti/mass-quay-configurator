FROM rust:1.76.0 AS builder
COPY . .
RUN cargo build --release

FROM registry.access.redhat.com/ubi9-micro
COPY --from=builder ./target/release/mqc ./target/release/mqc

ENTRYPOINT ["/target/release/mqc"]
