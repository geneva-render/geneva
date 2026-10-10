use crate::{ColorTags, Matrix, Primaries, Range, ResolvedTags, Transfer};

/// A default that was applied because a tag was missing.
///
/// Callers surface these as warnings so that a wrong guess is visible rather
/// than silent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inference {
    /// The tag that was missing: `primaries`, `transfer`, `matrix` or `range`.
    pub field: &'static str,
    /// The value that was assumed, in its JSON spelling.
    pub assumed: &'static str,
    /// Why that value was chosen.
    pub reason: &'static str,
}

/// Resolves missing tags to the most likely values for the material.
///
/// The rules encode long-standing industry defaults: untagged HD video is
/// BT.709, untagged SD video is BT.601, Y'CbCr video is limited range, and
/// R'G'B' material (matrix identity) is full range sRGB. Any tag that is
/// present is kept as is, even when it looks unusual.
pub fn infer(tags: ColorTags, width: u32, height: u32) -> (ResolvedTags, Vec<Inference>) {
    let mut notes = Vec::new();
    let is_hd = width >= 1280 || height >= 720;

    let matrix = tags.matrix.unwrap_or_else(|| {
        if is_hd {
            notes.push(Inference {
                field: "matrix",
                assumed: "bt709",
                reason: "untagged material at HD resolution or above",
            });
            Matrix::Bt709
        } else {
            notes.push(Inference {
                field: "matrix",
                assumed: "bt601",
                reason: "untagged material below HD resolution",
            });
            Matrix::Bt601
        }
    });

    let is_rgb = matrix == Matrix::Identity;

    let primaries = tags.primaries.unwrap_or_else(|| match matrix {
        Matrix::Bt601 => {
            // Both BT.601 variants are close; 625-line is the more common
            // untagged case in practice and the difference is small.
            notes.push(Inference {
                field: "primaries",
                assumed: "bt601-625",
                reason: "untagged material using the BT.601 matrix",
            });
            Primaries::Bt601_625
        }
        Matrix::Bt2020Ncl => {
            notes.push(Inference {
                field: "primaries",
                assumed: "bt2020",
                reason: "untagged material using the BT.2020 matrix",
            });
            Primaries::Bt2020
        }
        Matrix::Bt709 | Matrix::Identity => {
            notes.push(Inference {
                field: "primaries",
                assumed: "bt709",
                reason: if is_rgb {
                    "untagged R'G'B' material is assumed to be sRGB"
                } else {
                    "untagged material using the BT.709 matrix"
                },
            });
            Primaries::Bt709
        }
    });

    let transfer = tags.transfer.unwrap_or_else(|| {
        if is_rgb {
            notes.push(Inference {
                field: "transfer",
                assumed: "srgb",
                reason: "untagged R'G'B' material is assumed to be sRGB",
            });
            Transfer::Srgb
        } else {
            notes.push(Inference {
                field: "transfer",
                assumed: "bt709",
                reason: "untagged Y'CbCr material is assumed to be SDR video",
            });
            Transfer::Bt709
        }
    });

    let range = tags.range.unwrap_or_else(|| {
        if is_rgb {
            notes.push(Inference {
                field: "range",
                assumed: "full",
                reason: "R'G'B' material defaults to full range",
            });
            Range::Full
        } else {
            notes.push(Inference {
                field: "range",
                assumed: "limited",
                reason: "Y'CbCr video defaults to limited range",
            });
            Range::Limited
        }
    });

    (
        ResolvedTags {
            primaries,
            transfer,
            matrix,
            range,
        },
        notes,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untagged_hd_is_bt709_limited() {
        let (r, notes) = infer(ColorTags::default(), 1920, 1080);
        assert_eq!(r, ResolvedTags::SDR_VIDEO);
        assert_eq!(notes.len(), 4);
    }

    #[test]
    fn untagged_sd_is_bt601() {
        let (r, _) = infer(ColorTags::default(), 720, 576);
        assert_eq!(r.matrix, Matrix::Bt601);
        assert_eq!(r.primaries, Primaries::Bt601_625);
        assert_eq!(r.transfer, Transfer::Bt709);
        assert_eq!(r.range, Range::Limited);
    }

    #[test]
    fn rgb_material_is_srgb_full() {
        let tags = ColorTags {
            matrix: Some(Matrix::Identity),
            ..ColorTags::default()
        };
        let (r, notes) = infer(tags, 800, 600);
        assert_eq!(r, ResolvedTags::SRGB);
        assert_eq!(notes.len(), 3);
    }

    #[test]
    fn present_tags_are_preserved() {
        let tags = ColorTags {
            primaries: Some(Primaries::Bt2020),
            transfer: Some(Transfer::Pq),
            matrix: Some(Matrix::Bt2020Ncl),
            range: Some(Range::Full),
        };
        let (r, notes) = infer(tags, 3840, 2160);
        assert!(notes.is_empty(), "{notes:?}");
        assert!(r.is_hdr());
        assert_eq!(r.range, Range::Full);
    }
}
