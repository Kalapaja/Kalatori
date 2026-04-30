# Use Debian Bookworm for both stages to ensure glibc compatibility
FROM debian:bookworm-slim AS base

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

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none --profile minimal
ENV PATH="/root/.cargo/bin:${PATH}"

COPY rust-toolchain.toml ./

RUN rustup show active-toolchain || rustup toolchain install

# Install cargo-chef
RUN cargo install --locked cargo-chef

# Install subxt-cli separately to cache it, keep version in sync with Cargo.toml
RUN cargo install subxt-cli --version 0.44.0 --locked

FROM base AS planner

WORKDIR /usr/src/kalatori

COPY . .

RUN cargo chef prepare --recipe-path recipe.json


FROM base AS builder

WORKDIR /usr/src/kalatori

COPY rust-toolchain.toml ./

# Copy chef recipe
COPY --from=planner /usr/src/kalatori/recipe.json recipe.json

# Install cached deps
RUN cargo chef cook --release --recipe-path recipe.json

COPY Makefile front-end.mk ./

# Download front-end
RUN make download-front-end

# Download metadata
RUN make download-node-metadata-docker

COPY . .

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
