# Use an Ubuntu base image
FROM ubuntu:22.04 AS builder

# Install necessary dependencies
RUN apt-get update && \
    apt-get install -y \
    curl \
    build-essential \
    libssl-dev

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"
COPY . .
RUN cargo build --release

FROM ubuntu:22.04
RUN apt-get update && \
    apt-get install -y \
    curl \
    build-essential \
    libssl-dev

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"
COPY --from=builder ./target/release/mqc ./target/release/mqc

ENTRYPOINT ["/target/release/mqc"]
