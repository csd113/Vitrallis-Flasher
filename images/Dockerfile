# Pinned host-tool container for the Batch 1 deterministic image builder.
#
# This image exists only to compile and run the pinned sunxi-nand-image-builder
# against a deterministic entropy stream. It is never used by the desktop
# flasher, and the flasher never runs image construction or NAND operations.
#
# The rootfs construction route from `docs/image-build.md` (live-build against
# signed snapshots) was not selected; Batch 1 consumes the pinned prebuilt
# rootfs. The one fresh compilation step, the host-side SPL tool, is built here.
#
# Pins:
#   BASE_IMAGE       debian:bookworm-slim manifest-list digest (see inputs.lock.json)
#   DEBIAN_SNAPSHOT  signed snapshot.debian.org timestamp (see inputs.lock.json)
# Build with:
#   docker build --platform linux/amd64 \
#     --build-arg BASE_IMAGE=debian:bookworm-slim@sha256:<manifest-list> \
#     --build-arg DEBIAN_SNAPSHOT=<timestamp> -t vitrallis-flasher-builder .
# Run with --network none; only the pinned source tree is mounted read-only.
ARG BASE_IMAGE=debian:bookworm-slim@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251
FROM ${BASE_IMAGE}
ARG DEBIAN_SNAPSHOT=20260930T000000Z
# snapshot.debian.org is fetched over HTTP because the slim base image has no
# CA store before the first apt run; apt still verifies the signed InRelease
# against the pinned Debian archive keyring. `ca-certificates` is installed so
# later HTTPS use inside the image is possible.
RUN rm -f /etc/apt/sources.list \
 && printf 'Types: deb\nURIs: http://snapshot.debian.org/archive/debian/%s/\nSuites: bookworm\nComponents: main\nSigned-By: /usr/share/keyrings/debian-archive-keyring.gpg\n' "${DEBIAN_SNAPSHOT}" \
      > /etc/apt/sources.list.d/debian.sources \
 && apt-get update \
 && apt-get install -y --no-install-recommends gcc libc6-dev ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /build
