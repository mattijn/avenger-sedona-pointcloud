# Interactions for charts, September 2026

A catalogue for deciding what Avenger could support next, in three parts: the
ordinary interactions and what doing them *well* takes, the research-grade
techniques that no mainstream library ships yet, and magnifying, which this
repo built both ways in experiment 7.

How it was checked, on 25 Sep 2026: GitHub issues and PRs read with `gh`;
library features checked against their source or docs; every paper's DOI
resolved through Crossref and matched on title, authors and year. The papers'
full texts were mostly **not** opened: the one-line mechanisms are summaries,
not quotations. Anything unverified says so.

## Doing the ordinary ones well

The ordinary set is point and interval selection, pan and zoom, tooltips,
legend toggles, input bindings, overview + detail, and cross-filtering. Vega-Lite 6.4.3 (24 Apr 2026)
has `point` and `interval` selections, `translate`/`zoom`, and bindings to
inputs, legends and scales ([bind](https://vega.github.io/vega-lite/docs/bind.html)).
Lasso is an open PR ([#9723](https://github.com/vega/vega-lite/pull/9723)),
the segment brush an open issue and PR ([#9833](https://github.com/vega/vega-lite/issues/9833),
[#9849](https://github.com/vega/vega-lite/pull/9849)), and keyed animation
an open stack ending in [#9914](https://github.com/vega/vega-lite/pull/9914).

[vega/altair#3394](https://github.com/vega/altair/pull/3394) (open since
April 2024) tried to extend `.interactive()` with a tooltip and a clickable
legend. Its discussion is a good list of what makes ordinary interactions go
wrong:

- **Filter or fade depends on the chart.** In the Altair Express user study,
  fading worked for balanced scatterplots and failed for bars and outliers,
  where the axis does not rescale. Filtering rescales, which felt jumpy.
- **Users expect a selection to act on every view** in a compound chart.
- **Filtering upstream of a scale breaks its legend:** the colour domain is
  recomputed and entries vanish ([vega-lite#9360](https://github.com/vega/vega-lite/issues/9360));
  the fix was a parameter that holds the domain (`react: false`, Vega-Lite
  5.20, [#9374](https://github.com/vega/vega-lite/pull/9374)).
- **Two interactions fight over the same events:** releasing a pan cleared
  a legend selection, until the legend's events were filtered by mark name.
  Then a legend's symbol and its label turned out to be different targets.
- **A flag that silently does nothing is a trap** (`legend=True` with no
  legend to convert).

What follows for a selection API:
- the *effect* (filter, fade, recolour) is a declared choice separate from
  the *selection*, with a default per mark (filter bars, fade dense points);
- a selection has a scope, spec-wide by default;
- a filtering selection can freeze a scale's domain;
- gestures are routed with arbitration (a drag pans, a click selects), and a
  legend entry is one target;
- a convenience either takes effect or says why not, and never mutates its
  input.

## Research-grade techniques

Not in the mainstream libraries, as far as the source and tool lists of
Vega-Lite, Mosaic, Observable Plot, Bokeh, Plotly, ECharts and deck.gl show.
The last column says how each would compile, since Avenger's selections
become DataFusion predicates and its views are coordinate transforms.

| Technique | Mechanism | Primary reference | Compiles to |
|---|---|---|---|
| **Line brush** | A persistent line; every function graph that crosses it is selected. The precedent for vega-lite#9833 | Konyha et al., TVCG 2006, [10.1109/TVCG.2006.99](https://doi.org/10.1109/TVCG.2006.99); the case in #9833 is Collaris & van Wijk, *Contribution-Value plots*, VINCI 2020, [10.1145/3430036.3430067](https://doi.org/10.1145/3430036.3430067) | segment pairs by `LEAD(x), LEAD(y) OVER (PARTITION BY series ORDER BY t)`, a segment–segment intersection, `SELECT DISTINCT series` |
| **Crossing-based selection** | Stroke *across* targets instead of clicking them | Accot & Zhai, CHI 2002, [10.1145/503376.503390](https://doi.org/10.1145/503376.503390); Apitz & Guimbretière, CrossY, UIST 2004, [10.1145/1029632.1029635](https://doi.org/10.1145/1029632.1029635) | the line brush with a freehand stroke |
| **Angular brushing** | Select parallel-coordinate segments by their slope between two axes | Hauser, Ledermann, Doleisch, InfoVis 2002, [10.1109/INFVIS.2002.1173157](https://doi.org/10.1109/INFVIS.2002.1173157) | `atan2(b - a, dx) BETWEEN θ1 AND θ2`, a row predicate |
| **Timeboxes and angular queries** | Keep the series that stay inside a rectangle over its whole x-span; or by their rate of change | Hochheiser & Shneiderman, Information Visualization 2004, [10.1057/palgrave.ivs.9500061](https://doi.org/10.1057/palgrave.ivs.9500061) | `GROUP BY series HAVING bool_and(y BETWEEN y0 AND y1)` over the box's t-range |
| **Relaxed selection** | Sketch a pattern with a tolerance; nearby matches are selected | Holz & Feiner, UIST 2009, [10.1145/1622176.1622217](https://doi.org/10.1145/1622176.1622217) | distance to the sketch below a threshold (a UDF) |
| **Sketch-to-query** | Draw a shape; series are ranked by similarity. ShapeSearch adds an algebra that also takes natural language | Wattenberg, CHI EA 2001, [10.1145/634067.634292](https://doi.org/10.1145/634067.634292); Qetch, CHI 2018, [10.1145/3173574.3173962](https://doi.org/10.1145/3173574.3173962); ShapeSearch, SIGMOD 2020, [10.1145/3318464.3389722](https://doi.org/10.1145/3318464.3389722) | a similarity score column, `ORDER BY score LIMIT k` |
| **Smooth brushing** | A brush yields a degree of interest in [0, 1], not a yes or no | Doleisch & Hauser, J. WSCG 2002 ([record](https://dspace.zcu.cz/items/21a2b44c-1cda-4527-a248-449a96c733bd); no DOI, not opened) | a float column (distance fall-off) that drives opacity or a threshold |
| **Shape-adaptive brushes** | A drag becomes a selection that follows the data's distribution | Fan & Hauser, CGF 2018, [10.1111/cgf.13405](https://doi.org/10.1111/cgf.13405) | Mahalanobis: `(p-μ)ᵀΣ⁻¹(p-μ) < r²`, μ and Σ aggregated under the stroke |
| **Structured / percentile brushes** | A brush always holds a fixed count or percentile of points | Radoš et al., CGF 2016, [10.1111/cgf.12901](https://doi.org/10.1111/cgf.12901) | `ORDER BY dist(p, centre) LIMIT n` |
| **Brushing dimensions** | Brush over the attributes' statistics rather than the rows | Turkay, Filzmoser, Hauser, TVCG 2011, [10.1109/TVCG.2011.178](https://doi.org/10.1109/TVCG.2011.178) | a selection over per-column statistics |
| **Structure-aware 3D selection** | A 2D lasso on a point cloud selects the dense structure behind it, not everything on the line of sight (CloudLasso) | Yu et al., TVCG 2012, [10.1109/TVCG.2012.217](https://doi.org/10.1109/TVCG.2012.217) | points in the screen lasso, a voxel density `GROUP BY`, a threshold, a join back |
| **DimpVis hint paths** | Drag a mark along its own trajectory over time to scrub the whole chart through time | Kondo & Collins, TVCG 2014, [10.1109/TVCG.2014.2346250](https://doi.org/10.1109/TVCG.2014.2346250); [code](https://github.com/vialab/dimpVis) | a `t` parameter set by the drag; the trail is the rows of one key ordered by `t` |
| **ChronoLenses** | A lens that applies a local transform (a derivative, a correlation) to a time series under it | Zhao et al., TVCG 2011, [10.1109/TVCG.2011.195](https://doi.org/10.1109/TVCG.2011.195) | a region, a local pipeline, a coordinate transform |
| **Sampling lens** | A random sample of the points under the lens, so overplotting clears | Ellis, Bertini, Dix, CHI EA 2005, [10.1145/1056808.1056914](https://doi.org/10.1145/1056808.1056914) | `WHERE in_lens AND hash(id) % n = 0` |
| **Regression lens** | A local regression of the points under the lens, as it moves | Shao et al., CGF 2017, [10.1111/cgf.13176](https://doi.org/10.1111/cgf.13176) | `regr_slope`, `regr_intercept` over `in_lens` (both in DataFusion) |
| **MoleView** | A lens that pushes aside or reveals elements by an attribute | Hurter, Ersoy, Telea, TVCG 2011, [10.1109/TVCG.2011.223](https://doi.org/10.1109/TVCG.2011.223) | inside the region, filter or reorder by a predicate |
| **Mélange space folding** | Fold the space between two foci so both stay at full detail | Elmqvist et al., CHI 2008, [10.1145/1357054.1357263](https://doi.org/10.1145/1357054.1357263) | a piecewise coordinate transform, beside the fisheye |
| **Dust & Magnet** | Attribute magnets pull data points by their values | Yi et al., Information Visualization 2005, [10.1057/palgrave.ivs.9500099](https://doi.org/10.1057/palgrave.ivs.9500099) | a computed layout; a demonstration for keyed transitions |
| **Semantic interaction** (InterAxis, AxiSketcher) | Drag points or sketch on an axis, and the axis weights are solved to match | Endert et al., CHI 2012, [10.1145/2207676.2207741](https://doi.org/10.1145/2207676.2207741); Kim et al., TVCG 2016, [10.1109/TVCG.2015.2467615](https://doi.org/10.1109/TVCG.2015.2467615) | an axis `Σ wᵢ·colᵢ`, weights solved outside SQL |
| **Brush–pick–drop** (FromDaDy) | Brush trajectories, pick them up, drop them in another view to build a query | Hurter, Tissoires, Conversy, TVCG 2009, [10.1109/TVCG.2009.145](https://doi.org/10.1109/TVCG.2009.145) | named selections combined with `UNION`/`EXCEPT` |
| **Bubble cursor, excentric labels** | The pointer's target grows to the nearest mark; or labels fan out for every mark under it | Grossman & Balakrishnan, CHI 2005, [10.1145/1054972.1055012](https://doi.org/10.1145/1054972.1055012); Fekete & Plaisant, CHI 1999, [10.1145/302979.303148](https://doi.org/10.1145/302979.303148) | the nearest neighbour to the pointer |
| **Ambiguity widgets, predicate explanation** (DataTone, DimBridge) | Ambiguous words become editable widgets; a brush is explained back as a readable predicate | Gao et al., UIST 2015, [10.1145/2807442.2807478](https://doi.org/10.1145/2807442.2807478); Montambault et al., TVCG 2025, [10.1109/TVCG.2024.3456391](https://doi.org/10.1109/TVCG.2024.3456391) | classifier alternatives as parameters; a lasso summarised as a `WHERE` |

Also worth knowing: scented widgets ([10.1109/TVCG.2007.70589](https://doi.org/10.1109/TVCG.2007.70589)),
interactive legends ([10.1111/j.1467-8659.2009.01678.x](https://doi.org/10.1111/j.1467-8659.2009.01678.x)),
MyBrush, a model of brushing as source, link and target with a survey of
about thirty variants ([10.1109/TVCG.2017.2743859](https://doi.org/10.1109/TVCG.2017.2743859)),
and staggered transitions, which were found *not* to help tracking
([10.1109/TVCG.2014.2346424](https://doi.org/10.1109/TVCG.2014.2346424)), a
reason not to make staggering a default. Not found as a clear primary paper:
"sonified brushing", "similarity brushing" (Novotný & Hauser; no Crossref
match).

## Magnifying: a projection or a nested view

Two ways to implement it, and experiment 7 now has both
([README](../experiments/07-chart-decisions/README.md)):

- **As a coordinate transform (fisheye).** The Sarkar–Brown graphical
  fisheye ([10.1145/142750.142763](https://doi.org/10.1145/142750.142763)),
  after Furnas's degree of interest ([10.1145/22627.22342](https://doi.org/10.1145/22627.22342)),
  is one more point transform in experiment 5's family. One pass over the
  data; the context stays continuous; grid lines bend with it. But distances
  and slopes inside the lens are wrong, so magnitudes are misread. In the
  layer, symbol areas follow the transform's local magnification, so cells
  that touch on the flat map still touch under the lens.
- **As a nested view (magnifier).** A round inset over the focus, clipped
  with `Clip::Path`, draws the same items again, undistorted and scaled about
  the focus. Its scale is readable, but it covers what lies around it and the
  marks are drawn twice. This is the simplest magic lens (Bier et al. 1993,
  [10.1145/166117.166126](https://doi.org/10.1145/166117.166126); the lens
  survey by Tominski et al., CGF 2017, [10.1111/cgf.12871](https://doi.org/10.1111/cgf.12871),
  classifies lenses by where in the pipeline they act).

In the window the focus of either follows the cursor, and a click fixes it
in the pipeline.

## For Avenger, most promising

1. **Series predicates:** line brush, crossing, angular brush, timebox. All
   compile to window functions over `PARTITION BY series ORDER BY t` and a
   `GROUP BY series HAVING …`. Vega-Lite has row-level selections only; one
   series-level selection type would cover #9833 and twenty years of prior
   work.
2. **Fuzzy selections** (smooth brushing): a float column rather than a
   boolean, so interest animates through keyed transitions. It also settles
   the filter-or-fade question from #3394: the degree drives either.
3. **Lenses as a transform plus a local pipeline:** sampling, regression,
   MoleView, space folding, beside the fisheye and magnifier that exist now.
4. **A structure-aware lasso on the LiDAR tile:** screen lasso, voxel
   density, threshold. Measurable here, and not in any declarative library.
5. **Scrubbing by dragging a mark** (DimpVis): keyed marks across time already
   define each mark's trail.
6. **Text and sketch into one pipeline** (ShapeSearch, DataTone): the typed
   classifier and the writer of experiment 7 already turn text into a
   pipeline; ambiguity widgets and a sketch could refine it.
