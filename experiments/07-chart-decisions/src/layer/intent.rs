//! What a chart is for, as the Financial Times' Visual Vocabulary names it
//! (see `intents.md`): nine messages, each with the charts that serve it.
//! The round trip goes from a question's intent to the layer's chart kinds
//! that serve it, and from a chart kind back to the messages it can carry.

use serde_json::{json, Map, Value};

use super::model::Mark;

/// The FT's nine messages, with their descriptions and their charts, as the
/// chart-doctor repository lists them.
pub const MESSAGES: [(&str, &str, &[&str]); 9] = [
    ("deviation", "how far values depart from a reference, such as zero or an average",
        &["diverging bar", "diverging stacked bar", "spine", "surplus/deficit filled line"]),
    ("correlation", "how two or more variables relate",
        &["scatterplot", "line + column", "connected scatterplot", "bubble", "XY heatmap"]),
    ("ranking", "which comes first: an item's place in an ordered list matters more than its value",
        &["ordered bar", "ordered column", "ordered proportional symbol", "dot strip", "slope", "lollipop"]),
    ("distribution", "how values spread and how often each occurs",
        &["histogram", "boxplot", "violin", "population pyramid", "dot strip", "dot plot", "barcode", "cumulative curve"]),
    ("change_over_time", "how something changes over time: trends",
        &["line", "column", "line + column", "stock price", "slope", "area", "fan chart", "connected scatterplot", "calendar heatmap", "Priestley timeline", "circle timeline", "seismogram"]),
    ("part_to_whole", "how a whole breaks into its parts: shares",
        &["stacked column", "proportional stacked bar", "pie", "donut", "treemap", "Voronoi", "arc", "gridplot", "Venn", "waterfall"]),
    ("magnitude", "how big: comparing sizes",
        &["column", "bar", "paired column", "paired bar", "proportional stacked bar", "proportional symbol", "isotype", "lollipop", "radar", "parallel coordinates"]),
    ("spatial", "where: locations and patterns in space",
        &["choropleth", "proportional symbol", "flow map", "contour map", "equalised cartogram", "scaled cartogram", "dot density", "heat map"]),
    ("flow", "movement or volume between states or places",
        &["Sankey", "waterfall", "chord", "network"]),
];

/// The FT's name for each chart kind the layer draws. The bars are in class
/// order (`data.rs`), so they are the FT's `bar`, not its `ordered bar`; the
/// heatmap is class by height band, the FT's `XY heatmap`; the map is cells
/// at their coordinates, its `dot density`.
pub fn ft_chart(m: Mark) -> &'static str {
    match m {
        Mark::Bars => "bar",
        Mark::Pie => "donut",
        Mark::Line => "line",
        Mark::Heatmap => "XY heatmap",
        Mark::Map => "dot density",
    }
}

const LAYER_MARKS: [Mark; 5] = [Mark::Bars, Mark::Pie, Mark::Line, Mark::Heatmap, Mark::Map];

fn charts(intent: &str) -> &'static [&'static str] {
    MESSAGES.iter().find(|m| m.0 == intent).map_or(&[], |m| m.2)
}

/// Forward: the layer's chart kinds that serve an intent.
pub fn forward(intent: &str) -> Vec<Mark> {
    LAYER_MARKS.iter().copied().filter(|m| charts(intent).contains(&ft_chart(*m))).collect()
}

/// Back: the messages whose charts include a chart kind.
pub fn back(m: Mark) -> Vec<&'static str> {
    MESSAGES.iter().filter(|x| x.2.contains(&ft_chart(m))).map(|x| x.0).collect()
}

/// The trip closes when the chart is among those the intent leads to, and
/// the chart leads back to the intent.
pub fn closes(intent: &str, m: Mark) -> bool {
    forward(intent).contains(&m) && back(m).contains(&intent)
}

/// What the FT would draw that the layer lacks, for an intent it cannot serve.
pub fn missing(intent: &str) -> &'static [&'static str] {
    charts(intent)
}

/// The typed question for a decider: which of the nine the text expresses,
/// or none (an instruction about the chart, not a question of the data).
/// Its own question set, so the pilot's questions and their cache keys stay.
pub fn question() -> Value {
    let mut c: Map<String, Value> = MESSAGES.iter().map(|(k, d, _)| (k.to_string(), json!(d))).collect();
    c.insert("none".into(), json!("no question about the data: an instruction about how the chart looks, or none"));
    json!({"intent": {"type": "choice", "instructions": "Which question about the data does the instruction ask?", "criteria": c}})
}

/// The offline floor: keywords, as the `Rules` decider does for actions.
/// English only; `none` when nothing matches.
pub fn rules(text: &str) -> &'static str {
    let t = text.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| t.contains(w));
    if has(&["share", "proportion", "percent", "part of", "fraction"]) {
        "part_to_whole"
    } else if has(&["spread", "distribut", "range of", "how often"]) {
        "distribution"
    } else if has(&["over time", "trend", "change", "evolve", "grow"]) {
        "change_over_time"
    } else if has(&["where", "located", "location"]) {
        "spatial"
    } else if has(&["most", "least", "fewest", "biggest", "largest", "smallest", "top ", "rank", "highest", "lowest"]) && t.contains('?') {
        "ranking"
    } else if has(&["relat", "correlat", "versus", " vs ", "against"]) {
        "correlation"
    } else if has(&["above average", "below average", "deviat", "compared to the average"]) {
        "deviation"
    } else if has(&["flow", "from which", "moves"]) {
        "flow"
    } else if has(&["how many", "how much", "total", "compare"]) && t.contains('?') {
        "magnitude"
    } else {
        "none"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layers_charts_have_ft_names() {
        for m in LAYER_MARKS {
            assert!(!back(m).is_empty(), "{m:?} is in no message");
        }
    }

    #[test]
    fn round_trips() {
        assert!(closes("part_to_whole", Mark::Pie));
        assert!(closes("change_over_time", Mark::Line));
        assert!(closes("spatial", Mark::Map));
        assert!(!closes("distribution", Mark::Heatmap));
        assert!(forward("distribution").is_empty());
        assert!(!closes("ranking", Mark::Bars), "unordered bars are magnitude");
        assert!(!closes("ranking", Mark::Pie));
        assert_eq!(back(Mark::Bars), vec!["magnitude"]);
    }
}
