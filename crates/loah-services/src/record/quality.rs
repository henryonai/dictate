//! The quality of a screen recording: how often a frame is taken, how large the video may be, and how many bits go
//! to it. The names are the ones the picker shows; the numbers are what each name means on every system.

use serde::{Deserialize, Serialize};

/// What the user picks: small file, standard (the default), or sharp.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Small,
    #[default]
    Standard,
    Sharp,
}

/// The numbers behind a `Quality`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct QualitySpec {
    /// Frames per second of the video.
    pub fps: u32,
    /// The longest side of the video, in pixels: a larger screen is scaled down to it.
    pub max_long_edge: u32,
    /// Bits per second of the video.
    pub bitrate: u32,
}

impl Quality {
    pub fn spec(self) -> QualitySpec {
        match self {
            Quality::Small => QualitySpec {
                fps: 10,
                max_long_edge: 1280,
                bitrate: 1_000_000,
            },
            Quality::Standard => QualitySpec {
                fps: 15,
                max_long_edge: 1920,
                bitrate: 2_500_000,
            },
            Quality::Sharp => QualitySpec {
                fps: 30,
                max_long_edge: 3840,
                bitrate: 6_000_000,
            },
        }
    }
}

/// The size of the video of a screen of `source` pixels: the aspect ratio is kept, a screen is never scaled up, and
/// both sides are even (what the encoders take) and at least 2.
pub fn output_size(source: (u32, u32), spec: &QualitySpec) -> (u32, u32) {
    let (width, height) = (u64::from(source.0), u64::from(source.1));
    let long = width.max(height);
    let limit = u64::from(spec.max_long_edge);
    let (width, height) = if long > limit {
        // The long edge becomes the limit and the short one follows it, to the nearest pixel.
        let scaled = |side: u64| (side * limit + long / 2) / long;
        (scaled(width), scaled(height))
    } else {
        (width, height)
    };
    // Rounded down to even, so that the video is never larger than the screen.
    let even = |side: u64| (side & !1).max(2) as u32;
    (even(width), even(height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_size_keeps_the_aspect_and_is_even() {
        let standard = Quality::Standard.spec();
        let small = Quality::Small.spec();
        let sharp = Quality::Sharp.spec();

        // A Retina screen is scaled down to the long edge.
        assert_eq!(output_size((2880, 1800), &standard), (1920, 1200));
        // 719.65 is nearer 720 than 718, and 720 is even.
        assert_eq!(output_size((1366, 768), &small), (1280, 720));
        // A portrait screen is limited on its height, the long edge.
        assert_eq!(output_size((1800, 2880), &standard), (1200, 1920));
        // A screen that is under the limit is not scaled up; an odd side loses its odd pixel.
        assert_eq!(output_size((1001, 601), &sharp), (1000, 600));
        assert_eq!(output_size((1920, 1080), &sharp), (1920, 1080));
        // A strip keeps its aspect down to the 2 pixels that a video needs.
        assert_eq!(output_size((10_000, 10), &standard), (1920, 2));

        for source in [(2880, 1800), (1366, 768), (1001, 601), (3, 2), (1, 1)] {
            for spec in [small, standard, sharp] {
                let (w, h) = output_size(source, &spec);
                assert!(w % 2 == 0 && h % 2 == 0, "{source:?} gave {w}x{h}");
                assert!(w >= 2 && h >= 2, "{source:?} gave {w}x{h}");
                assert!(
                    w.max(h) <= spec.max_long_edge.max(2),
                    "{source:?} gave {w}x{h}"
                );
                // Never larger than the screen, unless the screen is under the 2 pixels that a video needs.
                assert!(w <= source.0.max(2) && h <= source.1.max(2));
            }
        }
    }

    #[test]
    fn the_presets_match_the_plan() {
        assert_eq!(
            Quality::Small.spec(),
            QualitySpec {
                fps: 10,
                max_long_edge: 1280,
                bitrate: 1_000_000
            }
        );
        assert_eq!(
            Quality::Standard.spec(),
            QualitySpec {
                fps: 15,
                max_long_edge: 1920,
                bitrate: 2_500_000
            }
        );
        assert_eq!(
            Quality::Sharp.spec(),
            QualitySpec {
                fps: 30,
                max_long_edge: 3840,
                bitrate: 6_000_000
            }
        );
        assert_eq!(Quality::default(), Quality::Standard);
    }

    #[test]
    fn a_quality_is_saved_by_its_name() {
        assert_eq!(serde_json::to_string(&Quality::Sharp).unwrap(), "\"sharp\"");
        assert_eq!(
            serde_json::from_str::<Quality>("\"small\"").unwrap(),
            Quality::Small
        );
    }
}
