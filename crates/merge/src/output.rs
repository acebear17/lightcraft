//! Merge results as linear DNGs (`PhotometricInterpretation = LinearRaw`, three samples per
//! pixel), written with LightCraft's own TIFF/DNG writer.
//!
//! - Pixel values are scaled so the brightest pixel sits just below 1.0 (the white level); the
//!   scale is carried in `BaselineExposure`, so a default rendering looks like the reference frame.
//! - Raw sources: the reference frame's DNG colour tags (`ColorMatrix*`, `ForwardMatrix*`,
//!   `CameraCalibration*`, `AsShotNeutral`, illuminants) are carried over — the data is still camera
//!   RGB. A raw with its white balance only in vendor multipliers (CR2, NEF…) has none of its own: the
//!   result's `AsShotNeutral` comes from the frames' multipliers ([`FrameColor::set_as_shot_multipliers`]).
//! - Other sources: the "camera" is the file's linear RGB space; `ColorMatrix1` = XYZ(D65) → RGB,
//!   `ForwardMatrix1` = RGB → XYZ(D50), `AsShotNeutral` = (1, 1, 1).
//! - Lens corrections (an HDR merge's: the reference frame's, [`crate::Frame::lens`]) go in `OpcodeList3`, where the
//!   develop settings' profile corrections find them as they do in the sources.

use lightcraft_color::{D50, D65, Mat3, bradford};
use lightcraft_geom::Orientation;
use lightcraft_meta::Metadata;
use lightcraft_raster::Rgb32f;
use lightcraft_raw::{BlackLevel, ColorData, DngCompression, DngWriteOptions, Opcode, OpcodeLists, RawData, RawFormat, RawImage, Rect};

use crate::frame::FrameColor;
use crate::{MergeError, Result};

/// Sample encoding of the output DNG.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DngSamples {
    /// 16-bit floating point, Deflate + floating-point predictor (HDR).
    Half,
    /// 32-bit floating point, Deflate + floating-point predictor.
    Float,
    /// 16-bit integer, lossless JPEG (panoramas of normal-range photos).
    U16,
}

/// The DNG colour tags for frames of `color`. A source's gain table map is dropped: it is
/// positioned on that one capture's active area, which a merged image (aligned, oriented,
/// stitched, cropped) no longer shares.
pub fn color_data(color: &FrameColor) -> ColorData {
    match color {
        FrameColor::Camera(c) => {
            let mut c = (**c).clone();
            c.profile.gain_table_map = None;
            c
        }
        FrameColor::Linear { to_xyz_d50 } => {
            let to_xyz_d65 = bradford(D50, D65).mul(to_xyz_d50);
            let cm = to_xyz_d65.inverse().unwrap_or(Mat3::IDENTITY);
            ColorData {
                illuminant: [21, 0],
                color_matrix: [Some(cm), None],
                forward_matrix: [Some(*to_xyz_d50), None],
                camera_calibration: [None, None],
                analog_balance: None,
                as_shot_neutral: Some([1.0, 1.0, 1.0]),
                as_shot_white_xy: None,
                baseline_exposure: 0.0,
                baseline_sharpness: None,
                profile: Default::default(),
                kelvin_presets: None,
            }
        }
    }
}

/// Encode `img` (scene-linear, 1.0 = source white) as a LinearRaw DNG, with `lens` (lens corrections for `img`, not
/// yet turned by `orientation`) as its `OpcodeList3`.
pub fn write_linear_dng(
    img: &Rgb32f,
    color: &FrameColor,
    orientation: Orientation,
    metadata: &Metadata,
    baseline_exposure: f64,
    samples: DngSamples,
    lens: &[Opcode],
) -> Result<Vec<u8>> {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return Err(MergeError::Output("empty image".into()));
    }
    let peak = img.data.iter().flat_map(|p| p.iter()).copied().filter(|v| v.is_finite()).fold(0.0f32, f32::max);
    let target = 0.95f32;
    // never brighten: values ≤ 1 keep their scale (so a plain panorama keeps its white level)
    let scale = if peak > target { target / peak } else { 1.0 };
    let mut cd = color_data(color);
    cd.baseline_exposure = baseline_exposure + (1.0 / scale as f64).log2();
    let clean = |v: f32| if v.is_finite() { (v * scale).max(0.0) } else { 0.0 };
    let (data, bits, white) = match samples {
        DngSamples::Half | DngSamples::Float => {
            (RawData::F32(img.data.iter().flat_map(|p| p.map(clean)).collect()), if samples == DngSamples::Half { 16 } else { 32 }, 1.0)
        }
        DngSamples::U16 => {
            (RawData::U16(img.data.iter().flat_map(|p| p.map(|v| (clean(v) * 65535.0).round().min(65535.0) as u16)).collect()), 16, 65535.0)
        }
    };
    let mut meta = metadata.clone();
    meta.width = Some(w as u32);
    meta.height = Some(h as u32);
    meta.orientation = Some(orientation);
    let raw = RawImage {
        format: RawFormat::Dng,
        width: w,
        height: h,
        cpp: 3,
        data,
        cfa: None,
        bits,
        black: BlackLevel::uniform(0.0),
        white: vec![white],
        active_area: Rect::new(0, 0, w, h),
        crop: Rect::new(0, 0, w, h),
        orientation,
        color: cd,
        wb_multipliers: None,
        linearized: false,
        pixel_aspect: 1.0,
        opcodes: OpcodeLists { list3: lens.to_vec(), ..Default::default() },
        metadata: meta,
        lens_metadata: Default::default(),
    };
    // Integer samples get lossless JPEG, the usual DNG compression for them: several raw readers take Deflate tiles
    // only as floats.
    let compression = match samples {
        DngSamples::Half => DngCompression::Deflate { tile: 256, half: true },
        DngSamples::Float => DngCompression::Deflate { tile: 256, half: false },
        DngSamples::U16 => DngCompression::Lj92 { tile: 256 },
    };
    lightcraft_raw::write_dng(&raw, &DngWriteOptions { compression, ..Default::default() }).map_err(|e| MergeError::Output(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_dng_round_trips_through_the_raw_decoder() {
        let img = Rgb32f::from_fn(80, 60, |x, y| [x as f32 / 20.0, y as f32 / 30.0, 0.3]);
        let color = FrameColor::Linear { to_xyz_d50: bradford(D65, D50).mul(&lightcraft_color::SRGB.to_xyz()) };
        let bytes = write_linear_dng(&img, &color, Orientation::Normal, &Metadata::default(), 0.0, DngSamples::Half, &[]).unwrap();
        let raw = lightcraft_raw::decode(&bytes).unwrap();
        assert_eq!((raw.width, raw.height, raw.cpp), (80, 60, 3));
        let gain = 2f32.powf(raw.color.baseline_exposure as f32);
        let back = raw.develop(lightcraft_raw::Method::Bilinear).unwrap();
        for (a, b) in img.data.iter().zip(&back.data) {
            for c in 0..3 {
                assert!((a[c] - b[c] * gain).abs() <= a[c] * 2e-3 + 1e-5, "{a:?} {b:?}");
            }
        }
        // the colour model maps the source white to neutral (no white-balance shift)
        let xy = lightcraft_raw::color::as_shot_white_xy(&raw);
        assert!((xy.x - D65.x).abs() < 2e-3 && (xy.y - D65.y).abs() < 2e-3, "{xy:?}");
        let t = lightcraft_raw::color::camera_transform(&raw, xy);
        let white = t.matrix.apply([t.wb[0] as f64, t.wb[1] as f64, t.wb[2] as f64]);
        assert!(white.iter().all(|v| (v - white[0]).abs() < 1e-3), "{white:?}");
        // sRGB red maps to the Rec.2020 coordinates of sRGB red
        let red = t.matrix.apply([t.wb[0] as f64, 0.0, 0.0]);
        let want = lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020).apply([1.0, 0.0, 0.0]);
        let k = white[0];
        for c in 0..3 {
            assert!((red[c] / k - want[c]).abs() < 5e-3, "{red:?} vs {want:?}");
        }
        let u16 = write_linear_dng(&img, &color, Orientation::Normal, &Metadata::default(), 0.0, DngSamples::U16, &[]).unwrap();
        assert!(lightcraft_raw::decode(&u16).is_ok());
    }

    /// Integer samples stay off Deflate, which several raw readers accept only for floats (#4838), and still store
    /// every value exactly: a width that isn't a whole number of tiles, black next to the brightest value.
    #[test]
    fn integer_samples_are_not_deflated_and_round_trip_exactly() {
        use craft_tiff::tags::{COMPRESSION, compression::ADOBE_DEFLATE};
        let img = Rgb32f::from_fn(300, 20, |x, y| if (x + y) % 2 == 0 { [0.0; 3] } else { [0.95, x as f32 / 400.0, y as f32 / 25.0] });
        let color = FrameColor::Linear { to_xyz_d50: bradford(D65, D50).mul(&lightcraft_color::SRGB.to_xyz()) };
        let write = |samples| write_linear_dng(&img, &color, Orientation::Normal, &Metadata::default(), 0.0, samples, &[]).unwrap();
        let compression = |bytes: &[u8]| craft_tiff::Tiff::parse(bytes).unwrap().ifds[0].u16(COMPRESSION);
        let (int, half) = (write(DngSamples::U16), write(DngSamples::Half));
        assert_ne!(compression(&int), Some(ADOBE_DEFLATE));
        assert_eq!(compression(&half), Some(ADOBE_DEFLATE));
        let want: Vec<u16> = img.data.iter().flat_map(|p| p.map(|v| (v * 65535.0).round() as u16)).collect();
        let RawData::U16(got) = lightcraft_raw::decode(&int).unwrap().data else { panic!("not integer samples") };
        assert!(got == want, "integer samples changed in the round trip");
    }
}
