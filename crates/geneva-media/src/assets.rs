use std::collections::HashMap;
use std::path::{Path, PathBuf};

use geneva_render::{AssetSource, FileAssets, Image, RenderError};
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
}

impl MediaAssets {
    /// Creates a loader resolving asset paths under `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            images: FileAssets::new(root.clone()),
            root,
            videos: HashMap::new(),
        }
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
                VideoReader::open(&self.root.join(&asset.src), asset.color).map_err(|e| {
                    RenderError::Asset {
                        id: id.to_owned(),
                        reason: e.to_string(),
                    }
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

    fn font(
        &mut self,
        comp: &Composition,
        id: &str,
    ) -> Result<std::sync::Arc<Vec<u8>>, RenderError> {
        self.images.font(comp, id)
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
