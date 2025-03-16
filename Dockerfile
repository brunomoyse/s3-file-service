# Stage 1: Build stage
FROM rust:1.84.1-alpine3.21 AS builder

# Install required dependencies
RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static pkgconfig

# Create a new project directory
WORKDIR /app

# Copy your source files
COPY ./src ./src
COPY Cargo.toml Cargo.lock ./

# Build the application
RUN cargo build --release

# Stage 2: Runtime stage
FROM alpine:3.21

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