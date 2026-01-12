FROM rust:slim-bookworm AS builder

RUN apt-get update && apt-get install -y \
    ca-certificates \
    pkg-config \
    build-essential \
    libssl-dev \
    protobuf-compiler \
  && rm -rf /var/lib/apt/lists/*

ADD . /app
WORKDIR /app

RUN cargo build -p pyth-lazer-agent --release --locked

FROM debian:12-slim

RUN apt update && apt install -y libssl-dev ca-certificates && apt clean all

COPY --from=builder /app/target/release/pyth-lazer-agent /pyth-lazer-agent/
COPY --from=builder /app/agent/config/* /pyth-lazer-agent/config/

ENTRYPOINT ["/pyth-lazer-agent/pyth-lazer-agent"]
