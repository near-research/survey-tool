FROM rust:bookworm AS builder

WORKDIR /app
COPY . .
RUN cargo build --release -p db-api

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/db-api /usr/local/bin/
RUN chmod 755 /usr/local/bin/db-api

RUN adduser --disabled-password --gecos '' dbapi
USER dbapi

EXPOSE 4001
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
  CMD curl -f http://localhost:4001/v1/health || exit 1
CMD ["db-api"]
