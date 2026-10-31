# Stage 1: Build stage
FROM rust:1.99.1-alpine3.24 AS builder

# nasm assembles the AVIF encoder and the TLS crypto library
RUN apk add --no-cache musl-dev nasm

WORKDIR /app

COPY ./src ./src
COPY Cargo.toml Cargo.lock ./

RUN cargo build --release --locked

# Stage 2: Runtime stage
FROM alpine:3.24.3

RUN apk add --no-cache ca-certificates

COPY --from=builder /app/target/release/s3-file-service /usr/local/bin/

WORKDIR /usr/local/bin

# Numeric uid of the nobody user, because Kubernetes expects one for pod security policies
USER 65534

EXPOSE 8000

CMD ["./s3-file-service"]
