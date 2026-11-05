# ARG: Prefer php:8.5-cli-bookworm. If unavailable, pin newest 8.x and rebuild.
ARG PHP_CLI_IMAGE=php:8.5-cli-bookworm
ARG RUST_IMAGE=rust:1.89-bookworm

FROM ${RUST_IMAGE} AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock* ./
COPY crates ./crates
RUN cargo build --release -p rearm-server

FROM ${PHP_CLI_IMAGE} AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
        $PHPIZE_DEPS \
        libpq-dev \
        libzip-dev \
        libssl-dev \
        libffi-dev \
        ca-certificates \
        curl \
        unzip \
        git \
    && docker-php-ext-install pdo_pgsql \
    && docker-php-ext-install pcntl \
    && docker-php-ext-install sockets \
    && docker-php-ext-configure ffi --with-ffi \
    && docker-php-ext-install ffi \
    && (docker-php-ext-enable opcache || true) \
    && pecl install msgpack \
    && docker-php-ext-enable msgpack \
    && apt-get purge -y --auto-remove $PHPIZE_DEPS \
    && rm -rf /tmp/pear /var/lib/apt/lists/*

COPY --from=composer:2 /usr/bin/composer /usr/bin/composer

RUN { \
      echo 'opcache.enable=1'; \
      echo 'opcache.enable_cli=1'; \
      echo 'opcache.memory_consumption=256'; \
      echo 'opcache.interned_strings_buffer=16'; \
      echo 'opcache.max_accelerated_files=20000'; \
      echo 'opcache.validate_timestamps=0'; \
      echo 'opcache.jit=0'; \
      echo 'opcache.jit_buffer_size=0'; \
      echo 'opcache.preload=/var/www/packages/octane-driver/preload.php'; \
      echo 'opcache.preload_user=root'; \
    } > /usr/local/etc/php/conf.d/opcache-recommended.ini

COPY --from=builder /src/target/release/rearm-server /usr/local/bin/rearm-server
RUN chmod +x /usr/local/bin/rearm-server

WORKDIR /var/www
RUN mkdir -p /var/www/uploads /var/www/certs

ENV REARM_HTTP_ADDR=0.0.0.0:8080 \
    REARM_HTTP3=false \
    REARM_HTTP3_ADDR=0.0.0.0:4433 \
    REARM_CERT=/var/www/certs/server.crt \
    REARM_KEY=/var/www/certs/server.key \
    REARM_UPLOAD_DIR=/var/www/uploads \
    REARM_APP_ROOT=/var/www/app \
    REARM_WORKER_SCRIPT=/var/www/packages/octane-driver/bin/worker.php \
    REARM_PHP_BIN=php

EXPOSE 8080 4433/udp
CMD ["rearm-server"]
