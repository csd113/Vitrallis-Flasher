# Rust compiler and signed Debian snapshot for the ARMv7 recovery daemon.
FROM rust:1.99.0-bookworm@sha256:2f5bd9fc62325d48f9fe8fe3dd1af7b7ea97ca16402c5aab2a29496bf1bec367
RUN rm -f /etc/apt/sources.list \
 && printf 'Types: deb\nURIs: http://snapshot.debian.org/archive/debian/20260930T000000Z/\nSuites: bookworm\nComponents: main\nSigned-By: /usr/share/keyrings/debian-archive-keyring.gpg\n' > /etc/apt/sources.list.d/debian.sources \
 && apt-get update \
 && apt-get install -y --no-install-recommends gcc-arm-linux-gnueabihf libc6-dev-armhf-cross qemu-user \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add armv7-unknown-linux-gnueabihf
ENV CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=arm-linux-gnueabihf-gcc \
    CC_armv7_unknown_linux_gnueabihf=arm-linux-gnueabihf-gcc \
    AR_armv7_unknown_linux_gnueabihf=arm-linux-gnueabihf-ar
WORKDIR /workspace
