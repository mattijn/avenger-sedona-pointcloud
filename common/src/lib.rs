//! The pieces every experiment needs: a DataFusion session that can read
//! LAS/LAZ, and the palette the charts share.

use std::sync::Arc;

use datafusion::execution::SessionStateBuilder;
use datafusion::prelude::{SessionConfig, SessionContext};
use sedona_pointcloud::las::format::{Extension, LasFormatFactory};
use sedona_pointcloud::las::options::LasOptions;

/// A session with SedonaDB's LAZ reader registered, so a tile can be queried
/// by path: `SELECT ... FROM 'tile.copc.laz'`.
pub fn las_context() -> SessionContext {
    las_context_with(SessionConfig::new())
}

/// The same, with room to set other session options first (batch size, for
/// instance).
pub fn las_context_with(config: SessionConfig) -> SessionContext {
    let config = config.with_option_extension(LasOptions::default());
    let mut state = SessionStateBuilder::new()
        .with_config(config)
        .with_default_features()
        .build();
    state
        .register_file_format(Arc::new(LasFormatFactory::new(Extension::Laz)), true)
        .expect("register the LAZ file format");
    SessionContext::new_with_state(state).enable_url_table()
}

/// The LiDAR classes these charts colour, as (code, label, colour).
pub const CLASSES: [(u8, &str, [f32; 4]); 5] = [
    (2, "Ground", [0.55, 0.43, 0.30, 1.0]),
    (3, "Low vegetation", [0.72, 0.84, 0.40, 1.0]),
    (4, "Medium vegetation", [0.35, 0.68, 0.33, 1.0]),
    (5, "High vegetation", [0.13, 0.43, 0.22, 1.0]),
    (6, "Building", [0.80, 0.25, 0.25, 1.0]),
];
/// Everything else: unclassified, water, bridges.
pub const OTHER: [f32; 4] = [0.62, 0.64, 0.68, 1.0];

pub const INK: [f32; 4] = [0.1, 0.12, 0.15, 1.0];
pub const MUTED: [f32; 4] = [0.38, 0.42, 0.47, 1.0];

// ---------------------------------------------------------------------------
// Number formatting for guides. Since avenger#139 a text engine formats
// numbers only when a formatter is configured; `default_text_engine()` has
// none, and the guides' short entry points use it. These wrappers keep the
// short signatures and pass an engine with D3 number formatting.

use avenger_guides::axis::opts::AxisConfig;
use avenger_guides::error::AvengerGuidesError;
use avenger_guides::legend::colorbar::ColorbarConfig;
use avenger_scales::scales::ConfiguredScale;
use avenger_scenegraph::marks::group::SceneGroup;

/// D3 number and date formatting, for scales and for text alike.
pub fn scale_formatting() -> avenger_scales::formatter::ScaleFormatting {
    avenger_scales::formatter::ScaleFormatting::d3(Default::default(), Default::default())
}

/// The default text engine with D3 number formatting, built once.
pub fn text_engine() -> &'static avenger_text::TextEngine {
    static ENGINE: std::sync::OnceLock<avenger_text::TextEngine> = std::sync::OnceLock::new();
    ENGINE.get_or_init(|| {
        scale_formatting().configure_text_engine(avenger_text::default_text_engine())
    })
}

/// `avenger_guides::axis::numeric::make_numeric_axis_marks`, with numbers formatted.
pub fn make_numeric_axis_marks(scale: &ConfiguredScale, title: &str, origin: [f32; 2], config: &AxisConfig) -> Result<SceneGroup, AvengerGuidesError> {
    // Since f4890be a scale carries its own label formatting as well.
    let scale = scale.clone().with_formatting(scale_formatting());
    avenger_guides::axis::numeric::make_numeric_axis_marks_with_text_engine(&scale, title, origin, config, text_engine())
}

/// `avenger_guides::legend::colorbar::make_colorbar_marks`, with numbers formatted.
pub fn make_colorbar_marks(scale: &ConfiguredScale, title: &str, origin: [f32; 2], config: &ColorbarConfig) -> Result<SceneGroup, AvengerGuidesError> {
    let scale = scale.clone().with_formatting(scale_formatting());
    avenger_guides::legend::colorbar::make_colorbar_marks_with_text_engine(&scale, title, origin, config, text_engine())
}

// ---------------------------------------------------------------------------
// Experiment 7's tilted view and CloudLasso, for the probe and experiment 11.

/// Experiment 7's tilt (layer/draw.rs `tilt`) over the window `win`
/// (x0, x1, y0, y1) with heights `hz`, as screen coefficients on a `plot` px
/// square: px = a[0] + a[1] x + a[2] y + a[3] z, and py likewise with b.
pub fn tilt(win: [f64; 4], hz: (f64, f64), yaw: f64, elevation: f64, plot: f64) -> ([f64; 4], [f64; 4]) {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (se, ce) = elevation.to_radians().sin_cos();
    let s = plot / std::f64::consts::SQRT_2 * 0.98;
    let (w, h, dz) = (win[1] - win[0], win[3] - win[2], hz.1 - hz.0);
    // Centred unit coordinates, and height at 0.35 of the plot.
    let x = [-win[0] / w - 0.5, 1.0 / w, 0.0, 0.0];
    let y = [-win[2] / h - 0.5, 0.0, 1.0 / h, 0.0];
    let z = [-0.35 * hz.0 / dz, 0.0, 0.0, 0.35 / dz];
    let (mut a, mut b) = ([0.0; 4], [0.0; 4]);
    for i in 0..4 {
        a[i] = s * (x[i] * cy - y[i] * sy);
        b[i] = -s * ((x[i] * sy + y[i] * cy) * se + z[i] * ce);
    }
    a[0] += plot / 2.0;
    b[0] += plot * 0.62;
    (a, b)
}

/// CloudLasso's region (layer/model.rs `lasso_take`): of the voxels whose
/// count reaches `structure` times the densest, joined across faces, edges
/// and corners, the region with the most points; and how many regions there
/// were.
pub fn largest_region(
    count: &std::collections::HashMap<(i64, i64, i64), usize>,
    structure: f64,
) -> (std::collections::HashSet<(i64, i64, i64)>, usize) {
    use std::collections::HashSet;
    let top = count.values().copied().max().unwrap_or(0);
    let dense: HashSet<_> = count.iter().filter(|(_, c)| **c as f64 >= structure * top as f64).map(|(v, _)| *v).collect();
    let (mut seen, mut best, mut best_n, mut regions) = (HashSet::new(), HashSet::new(), 0, 0);
    for v in &dense {
        if !seen.insert(*v) {
            continue;
        }
        regions += 1;
        let (mut stack, mut region, mut n) = (vec![*v], HashSet::new(), 0);
        while let Some(c) = stack.pop() {
            region.insert(c);
            n += count[&c];
            for d in 0..27 {
                let q = (c.0 + d % 3 - 1, c.1 + d / 3 % 3 - 1, c.2 + d / 9 - 1);
                if dense.contains(&q) && seen.insert(q) {
                    stack.push(q);
                }
            }
        }
        if n > best_n {
            (best, best_n) = (region, n);
        }
    }
    (best, regions)
}
