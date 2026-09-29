//! A histogram as keyed items, in two layouts: bars (x is the value, height
//! the count) and one stacked bar (x is the running share of the count), the
//! layout that bends into a donut.
//!
//! A value interval, such as a brush, is a rect in either layout. Its edges
//! sit at the same fraction of their bins in both, and `join` moves both ends
//! of every rect linearly, so during a transition the brush's edges stay
//! where they were within their bars. Stacking keeps the bins in order, so
//! the brush stays one rect: a band over the bars, an arc of the donut.

use crate::{Geo, Item};

/// Bin edges (n + 1, ascending) and counts (n).
#[derive(Clone, Debug)]
pub struct Bins {
    pub edges: Vec<f64>,
    pub counts: Vec<f64>,
}

/// How the bins sit in the unit square.
#[derive(Clone, Copy, Debug)]
pub enum Layout {
    /// Bars over the value axis, `max` count at the top.
    Bars { max: f64 },
    /// One bar across the plot, each bin as wide as its share, between `y`.
    Stack { y: [f64; 2] },
}

impl Bins {
    pub fn new(edges: Vec<f64>, counts: Vec<f64>) -> Self {
        assert_eq!(edges.len(), counts.len() + 1, "n + 1 edges for n counts");
        Self { edges, counts }
    }

    fn total(&self) -> f64 {
        self.counts.iter().sum::<f64>().max(f64::MIN_POSITIVE)
    }

    /// The count before bin `i`.
    fn before(&self, i: usize) -> f64 {
        self.counts[..i].iter().sum()
    }

    /// The bin holding `v`, and how far into it `v` lies; clamped to the ends.
    fn locate(&self, v: f64) -> (usize, f64) {
        let n = self.counts.len();
        let i = self.edges[1..n].partition_point(|e| *e <= v);
        let (lo, hi) = (self.edges[i], self.edges[i + 1]);
        (i, ((v - lo) / (hi - lo)).clamp(0.0, 1.0))
    }

    /// Unit x of value `v` in a layout.
    pub fn position(&self, v: f64, layout: Layout) -> f64 {
        match layout {
            Layout::Bars { .. } => {
                let (lo, hi) = (self.edges[0], *self.edges.last().unwrap());
                ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
            }
            Layout::Stack { .. } => {
                let (i, f) = self.locate(v);
                (self.before(i) + f * self.counts[i]) / self.total()
            }
        }
    }

    /// The value at unit x in a layout: the inverse of `position`, for a
    /// brush drawn on the layout. In the stacked layout an empty bin has no
    /// width, so no x falls in it.
    pub fn value_at(&self, u: f64, layout: Layout) -> f64 {
        let (lo, hi) = (self.edges[0], *self.edges.last().unwrap());
        match layout {
            Layout::Bars { .. } => lo + u.clamp(0.0, 1.0) * (hi - lo),
            Layout::Stack { .. } => {
                let target = u.clamp(0.0, 1.0) * self.total();
                let mut acc = 0.0;
                for (i, c) in self.counts.iter().enumerate() {
                    if *c > 0.0 && target <= acc + c {
                        let f = ((target - acc) / c).clamp(0.0, 1.0);
                        return self.edges[i] + f * (self.edges[i + 1] - self.edges[i]);
                    }
                    acc += c;
                }
                hi
            }
        }
    }

    /// Bin `i` as a unit rect `[x0, x1, y0, y1]`.
    pub fn rect(&self, i: usize, layout: Layout) -> [f64; 4] {
        match layout {
            Layout::Bars { max } => {
                let (x0, x1) = (self.position(self.edges[i], layout), self.position(self.edges[i + 1], layout));
                [x0, x1, 0.0, self.counts[i] / max.max(f64::MIN_POSITIVE)]
            }
            Layout::Stack { y } => {
                let n = self.total();
                let a = self.before(i);
                [a / n, (a + self.counts[i]) / n, y[0], y[1]]
            }
        }
    }

    /// Every bin as an item keyed `{prefix}{i}`, coloured by `fill`.
    pub fn items(&self, prefix: &str, layout: Layout, fill: impl Fn(usize) -> [f32; 4]) -> Vec<Item> {
        (0..self.counts.len())
            .map(|i| Item { key: format!("{prefix}{i}"), parent: None, geo: Geo::Rect(self.rect(i, layout)), fill: fill(i), size: 0.0, h: 0.0 })
            .collect()
    }

    /// The values `range` as one rect in a layout, from `y[0]` to `y[1]`.
    pub fn interval(&self, range: [f64; 2], layout: Layout, y: [f64; 2]) -> [f64; 4] {
        [self.position(range[0], layout), self.position(range[1], layout), y[0], y[1]]
    }

    /// The part of bin `i` whose values lie in `range`, as a unit rect with
    /// the bin's own height; `None` when they do not meet.
    pub fn clip(&self, i: usize, range: [f64; 2], layout: Layout) -> Option<[f64; 4]> {
        let (lo, hi) = (range[0].max(self.edges[i]), range[1].min(self.edges[i + 1]));
        if lo >= hi {
            return None;
        }
        let r = self.rect(i, layout);
        Some([self.position(lo, layout), self.position(hi, layout), r[2], r[3]])
    }

    /// Every bin's part in `range` as an item keyed `{prefix}{i}`.
    pub fn clipped(&self, prefix: &str, range: [f64; 2], layout: Layout, fill: [f32; 4]) -> Vec<Item> {
        (0..self.counts.len())
            .filter_map(|i| {
                let r = self.clip(i, range, layout)?;
                Some(Item { key: format!("{prefix}{i}"), parent: None, geo: Geo::Rect(r), fill, size: 0.0, h: 0.0 })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bins() -> Bins {
        Bins::new(vec![0.0, 10.0, 20.0, 30.0, 40.0], vec![5.0, 0.0, 15.0, 20.0])
    }

    #[test]
    fn stacked_shares_fill_the_plot() {
        let b = bins();
        let l = Layout::Stack { y: [0.35, 0.65] };
        assert_eq!(b.rect(0, l)[0], 0.0);
        assert!((b.rect(3, l)[1] - 1.0).abs() < 1e-12);
        for i in 0..3 {
            assert_eq!(b.rect(i, l)[1], b.rect(i + 1, l)[0], "bins {i} and {} touch", i + 1);
        }
    }

    #[test]
    fn value_at_undoes_position() {
        let b = bins();
        for l in [Layout::Bars { max: 20.0 }, Layout::Stack { y: [0.0, 1.0] }] {
            for v in [0.0, 3.0, 9.5, 21.0, 25.0, 33.3, 40.0] {
                let back = b.value_at(b.position(v, l), l);
                assert!((back - v).abs() < 1e-9, "{l:?}: {v} came back as {back}");
            }
        }
    }

    /// A brush edge keeps its place within its bin in both layouts, which is
    /// what lets `join` carry it along with the bars.
    #[test]
    fn a_brush_edge_stays_at_its_place_in_the_bin() {
        let b = bins();
        for l in [Layout::Bars { max: 20.0 }, Layout::Stack { y: [0.0, 1.0] }] {
            let r = b.rect(2, l);
            let x = b.position(24.0, l);
            assert!(((x - r[0]) / (r[1] - r[0]) - 0.4).abs() < 1e-12, "{l:?}");
        }
    }

    #[test]
    fn clip_takes_the_part_of_a_bin_in_range() {
        let b = bins();
        let l = Layout::Bars { max: 20.0 };
        assert_eq!(b.clip(2, [24.0, 100.0], l), Some([0.6, 0.75, 0.0, 0.75]));
        assert_eq!(b.clip(0, [24.0, 100.0], l), None);
        assert_eq!(b.clipped("s", [5.0, 25.0], l, [0.0; 4]).len(), 3);
    }
}
