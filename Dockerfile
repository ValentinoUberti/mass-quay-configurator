FROM rust:1.66 AS builder
COPY . .
RUN cargo build --release

FROM registry.access.redhat.com/ubi8-micro
COPY --from=builder ./target/release/mqc ./target/release/mqc
ENTRYPOINT ["/target/release/mqc"]
