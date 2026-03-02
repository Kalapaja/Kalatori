# Use Debian Bookworm for both stages to ensure glibc compatibility
FROM debian:bookworm-slim AS builder

# Install Rust and build dependencies
RUN apt-get update && apt-get install -y \
    build-essential \
    clang \
    pkg-config \
    ca-certificates \
    curl \
    libssl-dev \
    git \
    unzip \
    && rm -rf /var/lib/apt/lists/*

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.91
ENV PATH="/root/.cargo/bin:${PATH}"

# Remove old sqlite3 if present
RUN apt-get update && apt-get remove -y libsqlite3-0 libsqlite3-dev || true && rm -rf /var/lib/apt/lists/*

# Build and install SQLite 3.51.0 from source with required features for sqlx
WORKDIR /tmp
RUN curl -LO https://www.sqlite.org/2025/sqlite-autoconf-3510000.tar.gz \
    && tar xzf sqlite-autoconf-3510000.tar.gz \
    && cd sqlite-autoconf-3510000 \
    && CFLAGS="-DSQLITE_ENABLE_UNLOCK_NOTIFY=1 -DSQLITE_ENABLE_COLUMN_METADATA=1 -DSQLITE_ENABLE_DBSTAT_VTAB=1 -DSQLITE_ENABLE_FTS3=1 -DSQLITE_ENABLE_FTS3_PARENTHESIS=1 -DSQLITE_ENABLE_FTS5=1 -DSQLITE_ENABLE_JSON1=1 -DSQLITE_ENABLE_RTREE=1 -DSQLITE_ENABLE_STAT4=1" \
       ./configure --prefix=/usr/local --enable-shared --enable-static \
    && make -j$(nproc) \
    && make install \
    && ldconfig \
    && rm -rf /tmp/sqlite-autoconf*

# Create pkg-config file for sqlite3
RUN mkdir -p /usr/local/lib/pkgconfig && \
    cat > /usr/local/lib/pkgconfig/sqlite3.pc << 'EOF'
prefix=/usr/local
exec_prefix=${prefix}
libdir=${exec_prefix}/lib
includedir=${prefix}/include

Name: SQLite
Description: SQL database engine
Version: 3.51.0
Libs: -L${libdir} -lsqlite3
Libs.private: -lm -ldl -lpthread
Cflags: -I${includedir}
EOF

# Set environment for SQLite
ENV PKG_CONFIG_PATH=/usr/local/lib/pkgconfig
ENV LD_LIBRARY_PATH=/usr/local/lib
ENV SQLITE3_LIB_DIR=/usr/local/lib
ENV SQLITE3_INCLUDE_DIR=/usr/local/include

WORKDIR /usr/src/kalatori

# Install subxt-cli
COPY Makefile Makefile
RUN make install-subxt-cli

# Create source and examples directories
RUN mkdir -p daemon/src
RUN mkdir -p client/src
RUN mkdir -p client/examples

# Copy required Cargo.toml files
COPY Cargo.toml Cargo.toml
COPY daemon/Cargo.toml daemon/Cargo.toml
COPY client/Cargo.toml client/Cargo.toml
COPY Cargo.lock Cargo.lock

# Create dummy source and examples to cache dependencies
RUN echo "fn main() {}" > daemon/src/main.rs
RUN echo "fn lib() {}" > client/src/lib.rs
RUN echo "fn main() {}" > client/examples/crud.rs
RUN echo "fn main() {}" > client/examples/webhook.rs
RUN echo "fn main() {}" > client/examples/generate_hmac_test_vectors.rs

# Build dependencies only
RUN cargo build --release --all-features

# RUN cargo build --release -p kalatori-client --all-features

# Copy actual source code
COPY . .

# Download metadata
RUN make download-node-metadata-ci

# Download front-end
RUN make download-front-end

# Build the release binary
RUN CARGO_PROFILE_RELEASE_STRIP=false cargo build --release -p kalatori


# Runtime stage - use the same debian:bookworm-slim to ensure glibc compatibility
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy SQLite library from builder
COPY --from=builder /usr/local/lib/libsqlite3.so.0 /usr/local/lib/
RUN cd /usr/local/lib && ln -s libsqlite3.so.0 libsqlite3.so && ldconfig

# Copy the binary from builder
COPY --from=builder /usr/src/kalatori/target/release/kalatori /app/kalatori
COPY --from=builder /usr/src/kalatori/static /app/static

RUN useradd --no-create-home --system --uid 1000 kalatori \
    && chown kalatori:kalatori /app

USER kalatori

# Expose the default port
EXPOSE 8080

CMD ["/app/kalatori"]
