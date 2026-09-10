# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

- Media I/O: `geneva probe`, video decoding into the compositing format,
  `geneva render` to MP4/MOV/MKV/WebM with H.264 (OpenH264 or hardware),
  VP9, AV1 and AAC/Opus, audio mixing with gain and fades. Media libraries
  are built from pinned sources and linked statically.
- Text: shaping and layout for any script, font assets and system fonts,
  wrapping, alignment, per-word highlight, outline, shadow and background
  box.
- Reusable nested compositions.
- Video clips default to `fit: contain`; more blend modes.

- Timeline format 0.1: output settings, assets, visual layers with clips,
  transforms, keyframed properties with easing, text and shape sources, audio
  tracks.
- `geneva validate`, `geneva frame`, `geneva schema` commands.
- CPU reference renderer for solids, shapes and still images with
  linear-light compositing.
- Golden-frame test harness with per-channel tolerance, PSNR and SSIM.
