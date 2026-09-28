//! The gesture a selection came from, kept beside its tuples.
//!
//! Not upstream: added in this repo (VENDORED.md). Tuples are what a
//! selection means; they are not what was drawn. A lasso becomes runs of
//! cells, a line brush a set of keys, and neither gives back the outline a
//! chart draws over the data. A value can carry the gesture that made it,
//! and a contribution keeps it through `set`. `toggle` drops it, since after
//! adding or removing tuples the outline no longer describes them.

use std::hash::{Hash, Hasher};

/// What was drawn, in the chart's own terms: a kind (`"polygon"`,
/// `"segment"`, `"box"`, …), points in the units the chart drew them in, and
/// named numbers such as a soft width or CloudLasso's density share.
#[derive(Clone, Debug)]
pub struct Gesture {
    kind: String,
    points: Vec<[f64; 2]>,
    params: Vec<(String, f64)>,
    on: Vec<crate::ProjectionId>,
}

impl Gesture {
    pub fn new(kind: impl Into<String>, points: impl IntoIterator<Item = [f64; 2]>) -> Self {
        Self {
            kind: kind.into(),
            points: points.into_iter().collect(),
            params: Vec::new(),
            on: Vec::new(),
        }
    }
    /// Name the projections the points were drawn on, in the points' order:
    /// what a replay needs to draw the gesture again.
    pub fn on(mut self, projections: impl IntoIterator<Item = crate::ProjectionId>) -> Self {
        self.on = projections.into_iter().collect();
        self
    }
    pub fn projections(&self) -> &[crate::ProjectionId] {
        &self.on
    }
    /// Add a named number. A repeated name replaces the earlier value.
    pub fn with_param(mut self, name: impl Into<String>, value: f64) -> Self {
        let name = name.into();
        self.params.retain(|(n, _)| *n != name);
        self.params.push((name, value));
        self.params.sort_by(|a, b| a.0.cmp(&b.0));
        self
    }
    pub fn kind(&self) -> &str {
        &self.kind
    }
    pub fn points(&self) -> &[[f64; 2]] {
        &self.points
    }
    pub fn params(&self) -> &[(String, f64)] {
        &self.params
    }
    pub fn param(&self, name: &str) -> Option<f64> {
        self.params.iter().find(|(n, _)| n == name).map(|(_, v)| *v)
    }
    #[allow(clippy::type_complexity)]
    fn bits(
        &self,
    ) -> (
        &str,
        Vec<[u64; 2]>,
        Vec<(&str, u64)>,
        &[crate::ProjectionId],
    ) {
        (
            &self.kind,
            self.points
                .iter()
                .map(|p| [p[0].to_bits(), p[1].to_bits()])
                .collect(),
            self.params
                .iter()
                .map(|(n, v)| (n.as_str(), v.to_bits()))
                .collect(),
            &self.on,
        )
    }
}
// Bitwise, so a replayed gesture equals the one recorded.
impl PartialEq for Gesture {
    fn eq(&self, other: &Self) -> bool {
        self.bits() == other.bits()
    }
}
impl Eq for Gesture {}
impl Hash for Gesture {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bits().hash(state)
    }
}
