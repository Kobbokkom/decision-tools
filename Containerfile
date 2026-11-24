FROM docker.io/library/rust:1.93.1-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY migrations migrations
COPY assets/icon.png assets/icon.png
RUN cargo build --locked --release

FROM docker.io/library/debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates python3 \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /var/lib/decision-tools \
    && chown 10001:10001 /var/lib/decision-tools
COPY --from=build /build/target/release/decision-tools /usr/local/bin/decision-tools
COPY examples /opt/decision-tools/examples
USER 10001:10001
EXPOSE 8789
ENV DECISION_TOOLS_CONFIG=/opt/decision-tools/examples/workflows.toml \
    DECISION_TOOLS_DB=/var/lib/decision-tools/runs.sqlite3 \
    DECISION_TOOLS_LISTEN=0.0.0.0:8789
ENTRYPOINT ["/usr/local/bin/decision-tools"]
CMD ["serve"]
