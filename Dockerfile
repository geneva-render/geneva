# A geneva image, for farm workers in containers and on serverless
# functions. Built from this checkout on Ubuntu 22.04, which is where the
# release binaries' glibc 2.35 floor comes from.
#
#   docker build -t geneva .
#   docker run --rm -e GENEVA_FARM_URL -e GENEVA_FARM_TOKEN geneva
#
# With no arguments the container is a worker; with arguments it runs
# geneva with them (`docker run geneva render ...`). On AWS Lambda it
# answers invocations; see docs/farm.md.

FROM ubuntu:22.04 AS build
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update -qq \
 && apt-get install -y -qq build-essential cmake meson ninja-build nasm \
      pkg-config clang zlib1g-dev curl git ca-certificates >/dev/null
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal
# The media libraries change far less often than the code, so they are
# their own layer.
COPY scripts/build-media-libs.sh /src/scripts/
RUN MEDIA_SRC=/tmp/media-src /src/scripts/build-media-libs.sh /opt/media-libs \
 && rm -rf /tmp/media-src
COPY . /src
WORKDIR /src
RUN PKG_CONFIG_PATH=/opt/media-libs/lib/pkgconfig CARGO_TARGET_DIR=/tmp/target \
      cargo build --release --locked \
 && cp /tmp/target/release/geneva /usr/local/bin/geneva \
 && rm -rf /tmp/target

FROM ubuntu:22.04
ENV DEBIAN_FRONTEND=noninteractive
# x264 is loaded at run time when present; curl and jq are for the
# Lambda loop; the fonts are fallbacks for text in no font the timeline
# names.
RUN apt-get update -qq \
 && apt-get install -y -qq --no-install-recommends libx264-163 curl jq \
      ca-certificates fonts-dejavu-core >/dev/null \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/geneva /usr/local/bin/geneva
COPY --from=build /opt/media-libs/share/licenses /usr/share/licenses/geneva-media
COPY LICENSE THIRD-PARTY-NOTICES.md /usr/share/licenses/geneva/
COPY docker/entrypoint.sh /usr/local/bin/geneva-entrypoint
ENTRYPOINT ["geneva-entrypoint"]
