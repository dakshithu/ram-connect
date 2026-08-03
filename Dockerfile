# Multi-stage Dockerfile for cross-compiling static Linux binaries
FROM rust:1.75 as builder

# Install musl target for fully static Linux executables
RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Build stripped release binaries for Linux MUSL
RUN cargo build --release --target x86_64-unknown-linux-musl --bins

# Output stage: Copy binaries into a clean dist volume/folder
FROM alpine:latest
WORKDIR /dist
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/organizer ./organizer
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/contributor ./contributor
CMD ["cp", "/dist/organizer", "/dist/contributor", "/output/"]
