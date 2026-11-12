# syntax=docker/dockerfile:1.7
FROM rust:1.91-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY schema.sql ./schema.sql
COPY seed ./seed
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git --mount=type=cache,target=/build/target cargo build --release --locked && cp target/release/shopcore /usr/local/bin/shopcore

FROM python:3.13-slim-bookworm AS runtime
WORKDIR /srv/shopcore
COPY --from=build /usr/local/bin/shopcore /usr/local/bin/shopcore
COPY manage.py ./
COPY seed ./seed
ENV TZ=Asia/Jakarta PYTHONUNBUFFERED=1
EXPOSE 8000
CMD ["shopcore", "serve"]

FROM build AS dev
CMD ["cargo", "test", "--locked"]

FROM python:3.13-slim-bookworm AS tests
WORKDIR /tests
RUN pip install --no-cache-dir requests==2.32.5 pytest==8.4.2 psycopg2-binary==2.9.11
COPY tests ./
CMD ["pytest", "-q", "."]
