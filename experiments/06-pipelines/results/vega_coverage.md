## Corpus: 253 unique expressions

| Outcome | Expressions | Example |
|---|---|---|
| compiled, fails to type | 10 | `[bin_maxbins_10_IMDB_Rating_bins.start, bin_maxbins_10_IMDB_Rating_bins.stop]` |
| interaction (command) | 26 | `isValid(parent["country"]) ? parent["country"] : ""+parent["country"]` |
| reads the chart (query) | 34 | `bandspace(domain('x').length, 0.1, 0.05) * x_step` |
| row expression with signals, compiled | 67 | `width` |
| row expression, compiled | 102 | `isValid(datum["data"]) && isFinite(+datum["data"])` |
| unsupported | 14 | `"date (month): " + (timeFormat(datum["month_date"], timeUnitSpecifier(["month"], {"year-mo` |

### Reasons (an expression can have several)

- 10 × reads the chart: data()
- 10 × reads the chart: scale()
- 9 × unsupported: object literal
- 8 × interaction: x()
- 6 × reads the chart: bandspace()
- 6 × reads the chart: domain()
- 5 × interaction: y()
- 5 × type: Error during planning: array_element does not support type Float64. No function matches the gi
- 5 × type: Execution error: Cannot access field at argument 1: type Float64 is not Struct, Map, or Null
- 4 × reads the chart: invert()
- 4 × unsupported: `datum` as a whole object
- 3 × interaction: modify()
- 3 × interaction: parent
- 3 × interaction: vlSelectionResolve()
- 3 × reads the chart: bandwidth()
- 3 × unsupported: datetime()
- 2 × interaction: event
- 2 × interaction: panLinear()
- 2 × interaction: vlSelectionTest()
- 2 × interaction: zoomLinear()
- 2 × unsupported: sequence()
- 1 × interaction: group()
- 1 × interaction: isTuple()
- 1 × interaction: item()
- 1 × interaction: vlSelectionIdTest()

## Vega expression reference

| Section | Functions | Compile | Chart query | Interaction | Not yet |
|---|---|---|---|---|---|
| Type checking | 9 | 9 | 0 | 0 | – |
| Type coercion | 4 | 4 | 0 | 0 | – |
| Control flow | 1 | 1 | 0 | 0 | – |
| Math | 22 | 22 | 0 | 0 | – |
| Easing | 37 | 0 | 0 | 0 | easeLinear, easeQuad, easeQuadIn, easeQuadOut, easeQuadInOut, easeCubic, easeCubicIn, easeCubicOut, easeCubicInOut, easePoly, easePolyIn, easePolyOut, easePolyInOut, easeSin, easeSinIn, easeSinOut, easeSinInOut, easeExp, easeExpIn, easeExpOut, easeExpInOut, easeCircle, easeCircleIn, easeCircleOut, easeCircleInOut, easeBounce, easeBounceIn, easeBounceOut, easeBounceInOut, easeBack, easeBackIn, easeBackOut, easeBackInOut, easeElastic, easeElasticIn, easeElasticOut, easeElasticInOut |
| Statistical | 12 | 0 | 0 | 0 | sampleNormal, cumulativeNormal, densityNormal, quantileNormal, sampleLogNormal, cumulativeLogNormal, densityLogNormal, quantileLogNormal, sampleUniform, cumulativeUniform, densityUniform, quantileUniform |
| Date-time | 33 | 12 | 0 | 0 | datetime, week, isoweek, timezoneoffset, timeOffset, timeSequence, utc, utcdate, utcday, utcdayofyear, utcyear, utcquarter, utcmonth, utcweek, utcisoweek, utchours, utcminutes, utcseconds, utcmilliseconds, utcOffset, utcSequence |
| Array | 12 | 2 | 0 | 0 | extent, clampRange, join, lerp, interpolateLinear, peek, pluck, sequence, sort, span |
| String | 17 | 10 | 0 | 0 | lastindexof, pad, split, truncate, btoa, atob, encodeURIComponent |
| Object | 1 | 0 | 0 | 0 | merge |
| Formatting | 10 | 3 | 0 | 0 | dayFormat, dayAbbrevFormat, monthFormat, monthAbbrevFormat, timeUnitSpecifier, timeParse, utcParse |
| RegExp | 2 | 0 | 0 | 0 | regexp, test |
| Color | 6 | 0 | 0 | 0 | rgb, hsl, lab, hcl, luminance, contrast |
| Event | 8 | 0 | 0 | 6 | pinchAngle, inScope |
| Data | 2 | 0 | 2 | 0 | – |
| Scale and projection | 16 | 0 | 7 | 8 | gradient |
| Geographic | 5 | 0 | 4 | 0 | geoTranslate |
| Tree | 2 | 0 | 0 | 2 | – |
| Browser | 3 | 0 | 3 | 0 | – |
| Logging | 3 | 0 | 0 | 0 | warn, info, debug |
| Selection (Vega-Lite) | 4 | 0 | 0 | 4 | – |
| Constants | 11 | 11 | 0 | 0 | – |

74 of 220 documented names compile to DataFusion.
