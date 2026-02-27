ARG S3_PROVIDER
ARG S3_ACCESS_KEY_ID
ARG S3_SECRET_ACCESS_KEY
ARG S3_BUCKET_NAME
ARG S3_REGION

# Stage 1: Build stage
FROM rust:1.90-alpine3.22 AS builder

# Install required dependencies, including nasm
RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static pkgconfig nasm

# Create a new project directory
WORKDIR /app

# Copy your source files
COPY ./src ./src
COPY Cargo.toml Cargo.lock ./

ENV S3_PROVIDER=${S3_PROVIDER} \
    S3_ACCESS_KEY_ID=${S3_ACCESS_KEY_ID} \
    S3_SECRET_ACCESS_KEY=${S3_SECRET_ACCESS_KEY} \
    S3_BUCKET_NAME=${S3_BUCKET_NAME} \
    S3_REGION=${S3_REGION}

# Build the application
RUN cargo build --release

# Stage 2: Runtime stage
FROM alpine:3.22

# Install only necessary runtime dependencies
RUN apk add --no-cache ca-certificates

# Copy the compiled binary
COPY --from=builder /app/target/release/s3-file-service /usr/local/bin/

# Set the working directory
WORKDIR /usr/local/bin

# Switch to use a non-root user from here on
# Use uid of nobody user (65534) because kubernetes expects numeric user when applying pod security policies
USER 65534

# Command to run the executable
CMD ["./s3-file-service"]