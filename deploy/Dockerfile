# Container image for the OpenShogiPairings hosted server.
#
# Builds the server binary and the same-origin SPA, then ships both in a slim
# runtime. Build from the repo root:
#
#   docker build -f deploy/Dockerfile -t osp-server .
#   docker run -p 3000:3000 -e OSP_PASSWORD=your-password \
#     -v osp-data:/var/lib/osp osp-server
#
# Put Caddy (see Caddyfile) in front for TLS. OSP_PASSWORD must be supplied at
# run time — it is deliberately not baked into the image.

# --- Build the server binary ---
FROM rust:slim AS server-build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p osp-server

# --- Build the SPA, wired for same-origin (empty API base) ---
FROM node:22-slim AS spa-build
WORKDIR /spa
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend ./
ENV VITE_API_BASE=""
RUN npm run build

# --- Runtime ---
FROM debian:bookworm-slim
RUN useradd --system --user-group osp \
    && mkdir -p /var/lib/osp \
    && chown osp:osp /var/lib/osp
COPY --from=server-build /build/target/release/osp-server /usr/local/bin/osp-server
COPY --from=spa-build /spa/dist /opt/osp/dist
USER osp
# Bind all interfaces *inside the container*; the host/Caddy controls exposure.
ENV OSP_BIND=0.0.0.0:3000 \
    OSP_STATIC_DIR=/opt/osp/dist \
    OSP_DATA_FILE=/var/lib/osp/tournament.json
EXPOSE 3000
VOLUME ["/var/lib/osp"]
CMD ["osp-server"]
