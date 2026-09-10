# Third-party notices

Geneva's binaries include the following libraries, built from unmodified
pinned sources by `scripts/build-media-libs.sh` with only the components
Geneva uses. The core media libraries come from the FFmpeg project and are
built in a trimmed configuration for Geneva; the codecs are separate
projects. Their license texts are in `licenses/` and ship with every
binary distribution.

| Component | Purpose | License |
| --- | --- | --- |
| libavcodec, libavformat, libavutil, libswscale, libswresample (FFmpeg) | container reading and writing, decoding, pixel format and sample rate conversion | GNU LGPL 2.1 or later (`licenses/LGPL-2.1.txt`) |
| OpenH264 | H.264 encoding | BSD-2-Clause |
| libvpx | VP8/VP9 encoding | BSD-3-Clause, with patent grant |
| SVT-AV1 | AV1 encoding | BSD-2-Clause-Patent |
| dav1d | AV1 decoding | BSD-2-Clause |
| Opus | Opus encoding and decoding | BSD-3-Clause |
| nv-codec-headers | interface to NVIDIA hardware encoders (Linux) | MIT |

The LGPL-licensed libraries are statically linked. Their complete source,
the exact versions and the configuration used are given by the build
script in this repository, and Geneva itself is MIT licensed, so the
libraries can be modified and the program relinked as the LGPL provides.

Rust crates used by Geneva are listed with their licenses by
`cargo license` and are all MIT or Apache-2.0 licensed unless noted in
their manifests.
