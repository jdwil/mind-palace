# Build stage
FROM rust:1.96-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .

RUN cargo build --release -p mind-palace-dream

# Runtime stage — use same base to avoid network issues with separate apt
FROM rust:1.96-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/mind-palace-dream /usr/local/bin/mind-palace-dream

ENTRYPOINT ["mind-palace-dream"]
