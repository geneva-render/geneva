//! A temporary file of interleaved `f32` frames: where a treated mix
//! that is expensive to make (a denoised one) waits between the pass
//! that measures it and the pass that writes it, so it is made once.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;

use crate::MediaError;

pub(crate) struct Spool {
    path: PathBuf,
    writer: Option<BufWriter<File>>,
    reader: Option<BufReader<File>>,
}

fn io_err(what: &str, e: &std::io::Error) -> MediaError {
    MediaError::Codec {
        context: "audio spool".to_owned(),
        reason: format!("{what}: {e}"),
    }
}

impl Spool {
    /// A new, empty spool in the system's temporary directory.
    #[cfg_attr(not(feature = "denoise"), allow(dead_code))]
    pub(crate) fn new() -> Result<Self, MediaError> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path =
            std::env::temp_dir().join(format!("geneva-mix-{}-{nanos}.f32", std::process::id()));
        let file = File::create(&path).map_err(|e| io_err("creating", &e))?;
        Ok(Self {
            path,
            writer: Some(BufWriter::new(file)),
            reader: None,
        })
    }

    /// Appends interleaved stereo frames.
    pub(crate) fn write(&mut self, samples: &[f32]) -> Result<(), MediaError> {
        let Some(w) = self.writer.as_mut() else {
            return Err(io_err(
                "writing",
                &std::io::Error::other("spool already rewound"),
            ));
        };
        for s in samples {
            w.write_all(&s.to_le_bytes())
                .map_err(|e| io_err("writing", &e))?;
        }
        Ok(())
    }

    /// Goes back to the start for reading; also what to call between
    /// two readings.
    pub(crate) fn rewind(&mut self) -> Result<(), MediaError> {
        if let Some(mut w) = self.writer.take() {
            w.flush().map_err(|e| io_err("flushing", &e))?;
        }
        let file = File::open(&self.path).map_err(|e| io_err("opening", &e))?;
        self.reader = Some(BufReader::new(file));
        Ok(())
    }

    /// The next `frames` frames, fewer at the end, none after it.
    pub(crate) fn read(&mut self, frames: usize) -> Result<Vec<f32>, MediaError> {
        let Some(r) = self.reader.as_mut() else {
            return Ok(Vec::new());
        };
        let mut bytes = vec![0u8; frames * 2 * 4];
        let mut got = 0;
        while got < bytes.len() {
            let n = r
                .read(&mut bytes[got..])
                .map_err(|e| io_err("reading", &e))?;
            if n == 0 {
                break;
            }
            got += n;
        }
        Ok(bytes[..got - got % 8]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect())
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        self.writer = None;
        self.reader = None;
        let _ = std::fs::remove_file(&self.path);
    }
}
