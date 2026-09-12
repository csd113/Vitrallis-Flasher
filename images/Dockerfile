# Prepared context only: build.py requires an immutable arm/v7 container digest
# and a signed snapshot apt.sources. Never used by the desktop flasher.
ARG BASE_IMAGE
FROM ${BASE_IMAGE}
COPY apt.sources /etc/apt/sources.list.d/debian.sources
RUN ["apt-get", "update"]
RUN ["apt-get", "install", "-y", "--no-install-recommends", "live-build", "debootstrap", "apt-utils", "ca-certificates", "python3"]
WORKDIR /build/pocketchip
ENTRYPOINT ["lb", "build"]
