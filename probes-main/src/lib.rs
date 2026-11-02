//! The lidar_common guide helpers, rewritten for Avenger `main` (b5e3bce),
//! whose guides are prepared objects (`Axis`, `Colorbar`, `Legend`) that take
//! a length and place themselves in a slot.

use std::sync::Arc;

use avenger_guides::error::AvengerGuidesError;
use avenger_guides::legend::{Colorbar, ColorbarConfig, Legend, LegendConfig};
use avenger_guides::GuideSide;
use avenger_scales::scales::ConfiguredScale;
use avenger_scenegraph::marks::group::SceneGroup;
use avenger_typst_label::LabelEngine;

/// The bundled fonts with D3 number and date formatting, built once.
pub fn text_engine() -> &'static LabelEngine {
    static ENGINE: std::sync::OnceLock<LabelEngine> = std::sync::OnceLock::new();
    ENGINE.get_or_init(|| {
        avenger_typst_label::bundled_label_engine()
            .with_number_formatting(Arc::new(avenger_format_number_d3::D3NumberFormatProvider::new()))
            .with_datetime_formatting(Arc::new(avenger_format_datetime_d3::D3DateTimeFormatProvider::new()))
    })
}

/// A colorbar `length` long, placed by `Colorbar::origin` in `slot`
/// ([x, y, width, height]): there is no origin argument any more.
pub fn make_colorbar_marks(
    scale: &ConfiguredScale,
    title: &str,
    slot: [f32; 4],
    length: f32,
    config: &ColorbarConfig,
) -> Result<SceneGroup, AvengerGuidesError> {
    let colorbar = Colorbar::new(scale, title, config, text_engine())?;
    let mut group = colorbar.marks(length).group;
    group.origin = colorbar.origin(slot);
    Ok(group)
}

/// A colorbar on the right, with the default look.
pub fn colorbar_config() -> ColorbarConfig {
    let mut config = ColorbarConfig::default();
    config.axis.side = GuideSide::Right;
    config
}

/// A legend `length` long, placed by `Legend::origin` in `slot`.
pub fn make_symbol_legend(
    config: &LegendConfig,
    slot: [f32; 4],
    length: f32,
) -> Result<SceneGroup, AvengerGuidesError> {
    let legend = Legend::new(config, text_engine())?;
    let mut group = legend.marks(length).group;
    group.origin = legend.origin(slot);
    Ok(group)
}
