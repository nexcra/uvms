FROM rust:1.99.0-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake pkg-config python3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY ports ./ports
COPY src ./src
COPY scripts/prepare-nvidia-headers.py ./scripts/prepare-nvidia-headers.py
ARG NVIDIA=0
RUN if [ "$NVIDIA" = 1 ]; then \
      python3 scripts/prepare-nvidia-headers.py && \
      CUDA_INCLUDE_DIR=/opt/uvms-nvidia-sdk/nvidia/cuda_runtime/include cargo build --release --locked --features nvidia; \
    else cargo build --release --locked; fi

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /app uvms \
    && mkdir -p /var/lib/uvms/recordings && chown -R uvms:uvms /var/lib/uvms
WORKDIR /app
COPY --from=build /build/target/release/uvms-gateway /app/uvms-gateway
COPY web /app/web
USER 10001:10001
ENV UVMS_CONFIG=/etc/uvms/config.toml
EXPOSE 8080/tcp 8555/tcp 50000/udp
ENTRYPOINT ["/app/uvms-gateway"]
