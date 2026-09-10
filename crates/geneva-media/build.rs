//! Links the statically built media libraries for this crate's own test
//! binaries when the `media` feature is enabled.

fn main() {
    if std::env::var_os("CARGO_FEATURE_MEDIA").is_some() {
        geneva_media_link::link_media_libraries();
    }
}
