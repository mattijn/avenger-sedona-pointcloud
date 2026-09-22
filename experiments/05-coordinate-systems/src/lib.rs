//! Experiment 5: one coordinate-system trait for charts and maps.

pub mod coords;
pub mod draw;
pub mod specs;

/// Lambert-93 (EPSG:2154) metres to lon/lat degrees on GRS80, with the
/// IGN constants for the projection. Used to give the tile real geographic
/// coordinates, so the same cells can be drawn in `Spatial`.
pub fn lambert93_to_lonlat(x: f64, y: f64) -> (f64, f64) {
    const N: f64 = 0.725_607_765_053_267;
    const C: f64 = 11_754_255.426_096;
    const XS: f64 = 700_000.0;
    const YS: f64 = 12_655_612.049_876;
    const E: f64 = 0.081_819_191_042_815_8;
    const LON0: f64 = 3.0;
    let (dx, dy) = (x - XS, y - YS);
    let r = dx.hypot(dy);
    let gamma = (dx / -dy).atan();
    let lon = LON0 + gamma.to_degrees() / N;
    let l_iso = -(r / C).ln() / N;
    let mut phi = 2.0 * l_iso.exp().atan() - std::f64::consts::FRAC_PI_2;
    for _ in 0..12 {
        let es = E * phi.sin();
        phi = 2.0 * (((1.0 + es) / (1.0 - es)).powf(E / 2.0) * l_iso.exp()).atan()
            - std::f64::consts::FRAC_PI_2;
    }
    (lon, phi.to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IGN's reference point for Lambert-93: the projection origin at
    /// 3°E, 46.5°N is (700 000, 6 600 000).
    #[test]
    fn lambert93_origin() {
        let (lon, lat) = lambert93_to_lonlat(700_000.0, 6_600_000.0);
        assert!((lon - 3.0).abs() < 1e-9, "{lon}");
        assert!((lat - 46.5).abs() < 1e-7, "{lat}");
    }
}
