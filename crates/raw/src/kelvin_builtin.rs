//! Built-in white-balance presets for cameras whose raws carry neither colour matrices nor a preset table, feeding
//! the Kelvin scale of [`crate::kelvin_presets`].
//!
//! Nikon NEFs store only the as-shot multipliers (`WB_RBLevels`) and, in the maker note, the colour temperature the
//! camera states for the white-balance setting it shot with (`ColorTemperatureAuto`, `0x004f`: tag name from the
//! ExifTool Nikon tag documentation, read with exiftool run as a black-box tool on the files, or with our own TIFF
//! reader). A body's points are the as-shot multipliers of its own photos shot at one preset each, placed at the
//! temperature the camera itself wrote for that preset or, for a body that writes none for its presets, at the
//! approximate temperature Nikon's reference guide for that body lists. Presets off the daylight locus
//! (fluorescent, flash) and shots with a fine-tune shift are left out.
//!
//! Sources: own photos of each body listed (a reporter's samples, never committed), Nikon's own stated
//! temperatures (in the Z5II's files; for the Z fc, the White balance table of the Nikon Z fc Reference Guide,
//! <https://onlinemanual.nikonimglib.com/zfc/en/05-02.html>, numbers only), and the ExifTool Nikon tag-name
//! documentation (prose). No matrices or third-party data.
use crate::kelvin_presets::KelvinPresets;

/// One body: its make's first word, its Exif model without the make, and five (Kelvin, R/G, B/G) points.
struct Builtin {
    make: &'static str,
    model: &'static str,
    points: [(f32, f32, f32); 5],
}

const BUILTINS: &[Builtin] = &[
    // Nikon Z5II (issue emmertex/lightcraft-classic#157): Incandescent 3000 K, Direct sunlight 5210 K, the Kelvin
    // setting at 5500 K, Cloudy 5990 K, Shade 8000 K (the temperatures the camera writes in `ColorTemperatureAuto`).
    // Natural Auto (the camera wrote 5030 K) reads back at 5036 K; Flash (5410 K nominal) at about 6500 K.
    Builtin {
        make: "NIKON",
        model: "Z5_2",
        points: [
            (3000.0, 1.138_671_9, 2.083_984_4),
            (5210.0, 1.726_562_5, 1.335_937_5),
            (5500.0, 1.771_484_4, 1.281_25),
            (5990.0, 1.839_843_8, 1.212_890_6),
            (8000.0, 2.097_656_3, 1.089_843_8),
        ],
    },
    // Nikon Z fc (issue #4585): the same presets. The Z fc writes no `ColorTemperatureAuto` for a preset, so the
    // points sit at the approximate temperatures its reference guide lists: Incandescent 3000 K, Direct sunlight
    // 5200 K, Cloudy 6000 K, Shade 8000 K, and the Kelvin setting's 5500 K. Natural Auto (the camera wrote 5550 K)
    // reads back at about 5730 K with a slight magenta tint, the 5000 K Kelvin setting (with a one-step fine-tune)
    // at about 5070 K; Flash (5400 K listed) at about 6540 K, off this curve as on the Z5II.
    Builtin {
        make: "NIKON",
        model: "Z fc",
        points: [
            (3000.0, 1.226_562_5, 2.257_812_5),
            (5200.0, 1.882_812_5, 1.435_546_9),
            (5500.0, 1.935_546_9, 1.378_906_3),
            (6000.0, 2.015_625, 1.304_687_5),
            (8000.0, 2.318_359_4, 1.171_875),
        ],
    },
];

fn find_builtin(make: Option<&str>, model: &str) -> Option<&'static Builtin> {
    let model = model.trim();
    BUILTINS.iter().find(|b| {
        let n = b.make.len();
        let bare = match (model.get(..n), model.get(n..)) {
            (Some(prefix), Some(rest)) if prefix.eq_ignore_ascii_case(b.make) => rest.trim_start(),
            _ => model,
        };
        make.is_none_or(|m| m.split_whitespace().next().is_some_and(|w| w.eq_ignore_ascii_case(b.make))) && bare.eq_ignore_ascii_case(b.model)
    })
}

/// Whether this Exif make and model has built-in presets.
pub fn covers(make: Option<&str>, model: &str) -> bool {
    find_builtin(make, model).is_some()
}

/// The built-in presets of this Exif make and model, if any.
pub fn find(make: Option<&str>, model: &str) -> Option<KelvinPresets> {
    let b = find_builtin(make, model)?;
    KelvinPresets::new(b.points.map(|(k, r, b)| (k, [r, 1.0, b])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_makes_a_curve() {
        for b in BUILTINS {
            assert!(find(Some(b.make), b.model).is_some(), "{} {}", b.make, b.model);
        }
    }

    #[test]
    fn z5ii_reads_its_own_shots_near_the_camera_temperatures() {
        let p = find(Some("NIKON CORPORATION"), "NIKON Z5_2").unwrap();
        // Natural Auto, not on the curve: the camera wrote 5030 K for it
        let (t, tint) = p.temp_tint([1.695_312_5, 1.0, 1.373_046_9]).unwrap();
        assert!((1e6 / t - 1e6 / 5030.0).abs() < 10.0 && tint.abs() < 5.0, "{t} {tint}");
        // Flash (nominally 5410 K) sits near the curve on the daylight side
        let (t, tint) = p.temp_tint([1.916_015_6, 1.0, 1.164_062_5]).unwrap();
        assert!((5000.0..7500.0).contains(&t) && tint.abs() < 10.0, "{t} {tint}");
        // the 5990 K Kelvin shot with a green-magenta fine-tune shift: daylight, clearly tinted
        let (t, tint) = p.temp_tint([1.531_25, 1.0, 1.074_218_8]).unwrap();
        assert!((4000.0..7500.0).contains(&t) && tint.abs() > 20.0, "{t} {tint}");
    }

    #[test]
    fn z_fc_reads_its_own_shots_near_the_camera_temperatures() {
        let p = find(Some("NIKON CORPORATION"), "NIKON Z fc").unwrap();
        // the 5000 K Kelvin setting (not a point; shot with a one-step fine-tune)
        let (t, tint) = p.temp_tint([1.863_281_3, 1.0, 1.496_093_8]).unwrap();
        assert!((1e6 / t - 1e6 / 5000.0).abs() < 10.0 && tint.abs() < 10.0, "{t} {tint}");
        // Natural Auto, off the curve: the camera wrote 5550 K for it
        let (t, tint) = p.temp_tint([2.003_906_3, 1.0, 1.412_109_4]).unwrap();
        assert!((1e6 / t - 1e6 / 5550.0).abs() < 15.0 && tint.abs() < 15.0, "{t} {tint}");
        // Flash sits on the daylight side, and the settings keep their order: Incandescent, Natural Auto, Flash
        let (flash, tint) = p.temp_tint([2.111_328_1, 1.0, 1.255_859_4]).unwrap();
        assert!((5000.0..7500.0).contains(&flash) && tint.abs() < 10.0, "{flash} {tint}");
        let (incandescent, _) = p.temp_tint([1.226_562_5, 1.0, 2.257_812_5]).unwrap();
        assert!(incandescent < t && t < flash, "{incandescent} {t} {flash}");
    }

    #[test]
    fn matching() {
        assert!(covers(Some("NIKON CORPORATION"), "NIKON Z5_2") && covers(None, "Z5_2") && covers(Some("Nikon"), "nikon z5_2"));
        assert!(!covers(Some("NIKON CORPORATION"), "NIKON Z5") && !covers(Some("SONY"), "Z5_2") && !covers(None, "") && !covers(None, "NIK"));
        assert!(covers(Some("NIKON CORPORATION"), "NIKON Z fc") && covers(Some("Nikon"), "nikon z FC"));
        assert!(!covers(Some("NIKON CORPORATION"), "NIKON Z f") && !covers(Some("NIKON CORPORATION"), "NIKON Z 50") && !covers(None, "Zfc"));
    }
}
