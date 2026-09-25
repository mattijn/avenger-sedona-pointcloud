| Case | jev | haiku | haiku+jev | routed |
|---|---|---|---|---|
| v01 make the bars red | bars red | bars red | bars red | bars red |
| v02 which share does each class have? | pie | **✗** bars "Share of points per LiDAR class (%)" (mark) | pie | pie |
| v03 zoom in on the start of the lines, the top left | line zoom north_west | line zoom north_west | line zoom north_west | line zoom north_west |
| v04 mark the tallest buildings | map top 10 % | map top 10 % | map top 10 % | map top 10 % |
| v05 kleur de gebouwen groen | map green | map green | map green | map green |
| v06 nice, leave it like this | unchanged | unchanged | unchanged | unchanged |
| w01 call it Rooftops of the tile | **✗** unchanged (title) | map "Rooftops of the tile" | map "Rooftops of the tile" | map "Rooftops of the tile" |
| w02 noem de grafiek Punten per klasse | **✗** unchanged (title) | bars "Punten per klasse" | bars "Punten per klasse" | bars "Punten per klasse" |
| w03 only emphasise buildings taller than 70 m | **✗** unchanged (highlight, threshold) | map >= 70 | map >= 70 | map >= 70 |
| w04 use 10 m cells instead of 5 m | **✗** unchanged (rows) | map 3489 cells | map 3489 cells | map 3489 cells |
| w05 zoom to easting 657200 to 657600 and northing 6867300 to 6867700 | **✗** map zoom north_east (range) | map zoom custom | map zoom custom | map zoom custom |
| w06 zoom to the first 5 seconds | **✗** line zoom north_west (range_x) | line zoom custom | line zoom custom | line zoom custom |
| w07 make them orange and call it Point counts | **✗** bars orange (title) | bars "Point counts" orange | bars "Point counts" orange | bars "Point counts" orange |
| w08 back to bars in red, titled Classes | **✗** bars (colour, title) | bars "Classes" red | bars "Classes" red | bars "Classes" red |
| w09 colour the map green and emphasise everything above 60 m | **✗** map green (highlight, threshold) | map green >= 60 | map green >= 60 | map green >= 60 |
| w10 show the buildings in 2 m cells | **✗** unchanged (rows_gt) | map 63231 cells | map 63231 cells | map 63231 cells |
| w11 zoom to the north-east and emphasise buildings over 65 m | **✗** map zoom north_east (highlight, threshold) | map zoom north_east >= 65 | map zoom north_east >= 65 | map zoom north_east >= 65 |
| w12 draw the buildings as a 3D model | map | map | map | map |
| w13 emphasise the band with the most points | **✗** unchanged (selected) | **✗** unchanged (selected), 3 tries | heatmap, 3 tries | heatmap, 3 tries |
| w14 filter ground | **✗** unchanged (rows) | pie 1 cells | pie 1 cells | pie 1 cells |
| w15 only show ground and buildings | **✗** unchanged (rows) | bars 2 cells | bars 2 cells | bars 2 cells |
| w16 exclude building | **✗** unchanged (rows) | bars 5 cells | bars 5 cells | bars 5 cells |
| w17 show the local trend of each line between 15 and 25 seconds | line | line zoom custom, 2 tries | line zoom custom, 2 tries | line zoom custom, 2 tries |
| w18 look through the tall buildings in the middle of the map | map | map | map | map |
| w19 the north-east is crowded, show only a sample of the cells there | map | map | map | map |

| Arm | Right, vocabulary (v) | Right, own text (w) | Writer used | Accepted on the first try | Tries | Latency p50 / max | Cost |
|---|---|---|---|---|---|---|---|
| jev | 6/6 | 4/19 | 0 | – | 0 | 291 / 640 ms | $0.0020 |
| haiku | 5/6 | 18/19 | 25 | 23/25 | 28 | 1645 / 6958 ms | $0.1096 |
| haiku+jev | 6/6 | 19/19 | 25 | 23/25 | 28 | 2128 / 6247 ms | $0.1116 |
| routed | 6/6 | 19/19 | 20 | 18/20 | 23 | 1891 / 6247 ms | $0.0915 |

| Case | Start | Expected | Jev | Render | Confidence |
|---|---|---|---|---|---|
| b01 undo | bars | undo | undo | chart | 0.90 |
| b02 undo that | map | undo | undo | chart | 0.88 |
| b03 go back | pie | undo | undo | chart | 0.86 |
| b04 that was wrong, revert it | map | undo | undo | chart | 0.89 |
| b05 maak dat ongedaan | bars | undo | undo | chart | 0.89 |
| b06 terug naar hoe het was | line | undo | undo | chart | 0.42 |
| b07 start over | map | reset | reset | chart | 0.97 |
| b08 reset everything | heatmap | reset | reset | chart | 0.98 |
| b09 begin opnieuw | pie | reset | reset | chart | 0.95 |
| b10 reset the zoom | map | zoom | zoom/all | chart | 0.85 |
| b11 remove the emphasis | map | highlight | highlight/none | chart | 1.00 |
| b12 leave it as it was | bars | no_change | no_change | chart | 0.99 |
| b13 go back to the bar chart | line | mark | mark/bars | chart | 0.93 |
| r01 show the rows as a table | bars | no_change, table | **✗** mark/keep | table | 0.58 |
| r02 export this to parquet | map | no_change, export | no_change | export | 0.79 |
| r03 show the ground points as a table | bars | transform, table | **✗** mark/bars | table | 0.73 |
| r04 exclude building | pie | transform, chart | transform | chart | 1.00 |
| r05 make the bars red | bars | color, chart | color/red | chart | 1.00 |
| o01 what data is there? | bars | no_change, overview | no_change | overview | 0.75 |
| o02 welke kolommen heeft de tile? | map | no_change, overview | no_change | overview | 0.19 |
| o03 I don't know what I can do with this data, show me what's in it | pie | no_change, overview | **✗** mark/bars | overview | 0.64 |
| x01 magnify the north-east corner | map | view | view/magnifier | chart | 0.77 |
| x02 show the map in 3D | map | view | view/tilt | chart | 1.00 |
| x03 put a fisheye lens on the left | bars | view | view/fisheye | chart | 0.89 |
| x04 zoom in on the north-east | map | zoom | zoom/north_east | chart | 1.00 |

Jev only: 22/25 right.
