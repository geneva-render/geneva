use std::collections::HashMap;
use std::path::{Path, PathBuf};

use geneva_render::{AssetSource, FileAssets, Image, RenderError, VideoPlanes};
use geneva_timeline::{Composition, Ratio};

use crate::codecs::VideoReader;

/// Loads images and video frames from files under a root directory.
///
/// Video readers stay open between frames so that sequential rendering
/// decodes each source frame once.
pub struct MediaAssets {
    root: PathBuf,
    images: FileAssets,
    videos: HashMap<String, VideoReader>,
    /// Whether HDR video keeps its range (an HDR output) rather than
    /// being tone-mapped to SDR.
    keep_hdr: bool,
}

impl MediaAssets {
    /// Creates a loader resolving asset paths under `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            images: FileAssets::new(root.clone()),
            root,
            keep_hdr: false,
            videos: HashMap::new(),
        }
    }

    /// Keeps HDR video at its own range instead of tone-mapping it, for
    /// compositions whose output is HDR.
    pub fn keep_hdr(mut self, keep: bool) -> Self {
        self.keep_hdr = keep;
        self
    }

    /// The asset root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Opens (or returns the open) reader for a video asset.
    pub fn video(&mut self, comp: &Composition, id: &str) -> Result<&mut VideoReader, RenderError> {
        if !self.videos.contains_key(id) {
            let asset = comp.assets.get(id).ok_or_else(|| RenderError::Asset {
                id: id.to_owned(),
                reason: "not declared in the composition".to_owned(),
            })?;
            let reader =
                VideoReader::open_with(&self.root.join(&asset.src), asset.color, !self.keep_hdr)
                    .map_err(|e| RenderError::Asset {
                        id: id.to_owned(),
                        reason: e.to_string(),
                    })?;
            self.videos.insert(id.to_owned(), reader);
        }
        Ok(self.videos.get_mut(id).expect("inserted above"))
    }
}

impl AssetSource for MediaAssets {
    fn image(&mut self, comp: &Composition, id: &str) -> Result<&Image, RenderError> {
        self.images.image(comp, id)
    }

    fn image_at(&mut self, path: &str) -> Result<&Image, RenderError> {
        self.images.image_at(path)
    }

    fn font(
        &mut self,
        comp: &Composition,
        id: &str,
    ) -> Result<std::sync::Arc<Vec<u8>>, RenderError> {
        self.images.font(comp, id)
    }

    fn font_at(&mut self, path: &str) -> Result<std::sync::Arc<Vec<u8>>, RenderError> {
        self.images.font_at(path)
    }

    fn video_planes(
        &mut self,
        comp: &Composition,
        id: &str,
        source_time: Ratio,
    ) -> Result<Option<VideoPlanes<'_>>, RenderError> {
        let reader = self.video(comp, id)?;
        let planes = reader
            .planes_at(source_time)
            .map_err(|e| RenderError::Asset {
                id: id.to_owned(),
                reason: e.to_string(),
            })?;
        Ok(planes.map(|(p, width, height, tags)| VideoPlanes {
            width,
            height,
            y: p.y,
            cb: p.cb,
            cr: p.cr,
            y_stride: p.y_stride,
            c_stride: p.c_stride,
            tags,
        }))
    }

    fn video_size(
        &mut self,
        comp: &Composition,
        id: &str,
    ) -> Result<Option<(u32, u32)>, RenderError> {
        let reader = self.video(comp, id)?;
        Ok(Some((reader.width(), reader.height())))
    }

    fn video_frame_shrunk(
        &mut self,
        comp: &Composition,
        id: &str,
        source_time: Ratio,
        size: [u32; 2],
    ) -> Result<&Image, RenderError> {
        let reader = self.video(comp, id)?;
        reader
            .frame_at_shrunk(source_time, size)
            .map_err(|e| RenderError::Asset {
                id: id.to_owned(),
                reason: e.to_string(),
            })
    }

    fn video_frame(
        &mut self,
        comp: &Composition,
        id: &str,
        source_time: Ratio,
    ) -> Result<&Image, RenderError> {
        let reader = self.video(comp, id)?;
        reader
            .frame_at(source_time)
            .map_err(|e| RenderError::Asset {
                id: id.to_owned(),
                reason: e.to_string(),
            })
    }
}
