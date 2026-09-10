//! Mapping between the media libraries' color enums and `geneva-color` tags.

use ffmpeg_next::util::color;
use geneva_color::{ColorTags, Matrix, Primaries, Range, ResolvedTags, Transfer};

pub fn from_codec_tags(
    space: color::Space,
    range: color::Range,
    primaries: color::Primaries,
    transfer: color::TransferCharacteristic,
) -> ColorTags {
    ColorTags {
        primaries: match primaries {
            color::Primaries::BT709 => Some(Primaries::Bt709),
            color::Primaries::BT470BG => Some(Primaries::Bt601_625),
            color::Primaries::SMPTE170M | color::Primaries::SMPTE240M => Some(Primaries::Bt601_525),
            color::Primaries::BT2020 => Some(Primaries::Bt2020),
            _ => None,
        },
        transfer: match transfer {
            color::TransferCharacteristic::BT709
            | color::TransferCharacteristic::SMPTE170M
            | color::TransferCharacteristic::BT2020_10
            | color::TransferCharacteristic::BT2020_12 => Some(Transfer::Bt709),
            color::TransferCharacteristic::IEC61966_2_1 => Some(Transfer::Srgb),
            color::TransferCharacteristic::Linear => Some(Transfer::Linear),
            color::TransferCharacteristic::SMPTE2084 => Some(Transfer::Pq),
            color::TransferCharacteristic::ARIB_STD_B67 => Some(Transfer::Hlg),
            _ => None,
        },
        matrix: match space {
            color::Space::BT709 => Some(Matrix::Bt709),
            color::Space::BT470BG | color::Space::SMPTE170M | color::Space::SMPTE240M => {
                Some(Matrix::Bt601)
            }
            color::Space::BT2020NCL => Some(Matrix::Bt2020Ncl),
            color::Space::RGB => Some(Matrix::Identity),
            _ => None,
        },
        range: match range {
            color::Range::MPEG => Some(Range::Limited),
            color::Range::JPEG => Some(Range::Full),
            color::Range::Unspecified => None,
        },
    }
}

pub fn to_codec_tags(
    tags: ResolvedTags,
) -> (
    color::Space,
    color::Range,
    color::Primaries,
    color::TransferCharacteristic,
) {
    let space = match tags.matrix {
        Matrix::Bt709 => color::Space::BT709,
        Matrix::Bt601 => color::Space::SMPTE170M,
        Matrix::Bt2020Ncl => color::Space::BT2020NCL,
        Matrix::Identity => color::Space::RGB,
    };
    let range = match tags.range {
        Range::Limited => color::Range::MPEG,
        Range::Full => color::Range::JPEG,
    };
    let primaries = match tags.primaries {
        Primaries::Bt709 => color::Primaries::BT709,
        Primaries::Bt601_625 => color::Primaries::BT470BG,
        Primaries::Bt601_525 => color::Primaries::SMPTE170M,
        Primaries::Bt2020 => color::Primaries::BT2020,
    };
    let transfer = match tags.transfer {
        Transfer::Bt709 => color::TransferCharacteristic::BT709,
        Transfer::Srgb => color::TransferCharacteristic::IEC61966_2_1,
        Transfer::Linear => color::TransferCharacteristic::Linear,
        Transfer::Pq => color::TransferCharacteristic::SMPTE2084,
        Transfer::Hlg => color::TransferCharacteristic::ARIB_STD_B67,
    };
    (space, range, primaries, transfer)
}
