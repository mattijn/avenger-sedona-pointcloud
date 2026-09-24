| Case | rules | jev | llm |
|---|---|---|---|
| d01 more rows of the same kind (free) | no_change | **✗** title (0.12) | **✗** mark/map |
| d01 more rows of the same kind (narrow) | no_change | **✗** zoom/keep (0.26) | no_change |
| d02 coordinates and a geometry column appear (free) | mark/map | mark/map (0.83) | mark/map |
| d02 coordinates and a geometry column appear (narrow) | no_change | no_change (0.27) | no_change |
| d03 an outlier appears (free) | highlight/outliers | highlight/outliers (0.84) | highlight/outliers |
| d03 an outlier appears (narrow) | highlight/outliers | highlight/outliers (0.57) | highlight/outliers |
| d04 a new category appears (free) | no_change | **✗** highlight/outliers (0.75) | **✗** highlight/outliers |
| d04 a new category appears (narrow) | no_change | **✗** zoom/keep (0.39) | no_change |
| i01 make the bars red | color/red | color/red (0.99) | color/red |
| i02 maak de balken rood | **✗** no_change | color/red (0.99) | color/red |
| i03 orange would look better | color/orange | color/orange (0.78) | color/orange |
| i04 the labels overlap, tilt them | rotate_labels/tilt | rotate_labels/tilt (0.99) | rotate_labels/tilt |
| i05 put the labels upright | **✗** no_change | rotate_labels/vertical (0.97) | rotate_labels/vertical |
| i06 turn this into a scatter plot | mark/points | mark/points (0.99) | mark/points |
| i07 rename the chart to Building heights | **✗** no_change | title (0.97) | title |
| i08 looks good, don't change anything | no_change | no_change (0.72) | no_change |
| i09 the colour is too loud, something calmer please | **✗** no_change | color/blue (0.99) | color/grey |
| i10 zoom to the north-east | zoom/north_east | zoom/north_east (0.99) | zoom/north_east |
| i11 show me the top left corner | zoom/north_west | zoom/north_west (0.80) | zoom/north_west |
| i12 can we look closer at the lower right part | **✗** no_change | zoom/south_east (1.00) | zoom/south_east |
| i13 go back to the whole tile | zoom/all | zoom/all (0.93) | zoom/all |
| i14 highlight the tallest buildings | highlight/top_10 | highlight/top_10 (0.94) | highlight/top_10 |
| i15 where are the outliers? | highlight/outliers | highlight/outliers (0.51) | highlight/outliers |
| i16 inzoomen op het zuidwesten | **✗** no_change | zoom/south_west (0.99) | zoom/south_west |
| i17 make the points green | color/green | color/green (1.00) | color/green |
| i18 hmm | no_change | **✗** mark/map (0.23) | no_change |
| i19 turn this into a scatter plot | no_change | no_change (0.82) | no_change |

| Decider | Instructions correct | Data changes correct | Latency p50 / p95 | Cost, all decisions | Input tokens (mean) | Accepted / needs text / unfit / rejected |
|---|---|---|---|---|---|---|
| rules | 13/19 | 8/8 | 0 / 0 ms | $0.00000 | 0 | 13 / 0 / 0 / 0 |
| jev | 18/19 | 4/8 | 290 / 680 ms | $0.00129 | 1135 | 20 / 2 / 2 / 0 |
| llm | 19/19 | 6/8 | 1114 / 2315 ms | $0.03319 | 911 | 20 / 1 / 0 / 0 |
