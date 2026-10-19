# Built by GitHub Actions (.github/workflows/ci.yml) and run by Coolify.
# cargo-chef builds the dependencies in a layer of their own, so a change to
# our code doesn't rebuild them.

FROM lukemathwalker/cargo-chef:latest-rust-1-trixie AS chef
WORKDIR /src

FROM chef AS plan
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=plan /src/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked

FROM debian:trixie-slim
# ca-certificates for HTTPS to the bucket; curl for Coolify's health check.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 api
COPY --from=build /src/target/release/hasteclip-api /usr/local/bin/hasteclip-api
USER api
ENV PORT=8080
EXPOSE 8080
CMD ["hasteclip-api"]
