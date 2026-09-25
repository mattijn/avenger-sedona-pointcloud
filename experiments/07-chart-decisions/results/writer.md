| Case | jev | haiku | haiku+jev | routed |
|---|---|---|---|---|
| v01 make the bars red | bars red | bars red | bars red | bars red |
| v02 which share does each class have? | pie | pie | pie | pie |
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
| w12 draw the buildings as a 3D model | unchanged | map 3316187 cells | map 3316155 cells | map 3316155 cells |
| w13 emphasise the band with the most points | unchanged | unchanged, 2 tries | unchanged, 3 tries | unchanged, 3 tries |
| w14 filter ground | **✗** unchanged (rows) | pie 1 cells | pie 1 cells | pie 1 cells |
| w15 only show ground and buildings | **✗** unchanged (rows) | bars 2 cells | bars 2 cells | bars 2 cells |
| w16 exclude building | **✗** unchanged (rows) | bars 5 cells | bars 5 cells | bars 5 cells |

| Arm | Right, vocabulary (v) | Right, own text (w) | Writer used | Accepted on the first try | Tries | Latency p50 / max | Cost |
|---|---|---|---|---|---|---|---|
| jev | 6/6 | 2/16 | 0 | – | 0 | 302 / 699 ms | $0.0013 |
| haiku | 6/6 | 16/16 | 22 | 21/22 | 23 | 1445 / 3859 ms | $0.0643 |
| haiku+jev | 6/6 | 16/16 | 22 | 21/22 | 24 | 1795 / 5376 ms | $0.0704 |
| routed | 6/6 | 16/16 | 18 | 17/18 | 20 | 1692 / 5376 ms | $0.0581 |

| Case | Start | Expected | Jev | Render | Confidence |
|---|---|---|---|---|---|
| b01 undo | bars | undo | undo | chart | 0.89 |
| b02 undo that | map | undo | undo | chart | 0.89 |
| b03 go back | pie | undo | undo | chart | 0.88 |
| b04 that was wrong, revert it | map | undo | undo | chart | 0.91 |
| b05 maak dat ongedaan | bars | undo | undo | chart | 0.91 |
| b06 terug naar hoe het was | line | undo | undo | chart | 0.58 |
| b07 start over | map | reset | reset | chart | 0.94 |
| b08 reset everything | heatmap | reset | reset | chart | 0.97 |
| b09 begin opnieuw | pie | reset | reset | chart | 0.93 |
| b10 reset the zoom | map | zoom | zoom/all | chart | 0.80 |
| b11 remove the emphasis | map | highlight | highlight/none | chart | 1.00 |
| b12 leave it as it was | bars | no_change | no_change | chart | 0.98 |
| b13 go back to the bar chart | line | mark | mark/bars | chart | 0.90 |
| r01 show the rows as a table | bars | no_change, table | **✗** mark/keep | table | 0.82 |
| r02 export this to parquet | map | no_change, export | no_change | export | 0.67 |
| r03 show the ground points as a table | bars | transform, table | **✗** mark/bars | table | 0.68 |
| r04 exclude building | pie | transform, chart | transform | chart | 1.00 |
| r05 make the bars red | bars | color, chart | color/red | chart | 1.00 |
| o01 what data is there? | bars | no_change, overview | no_change | overview | 0.72 |
| o02 welke kolommen heeft de tile? | map | no_change, overview | no_change | overview | 0.24 |
| o03 I don't know what I can do with this data, show me what's in it | pie | no_change, overview | **✗** mark/bars | overview | 0.68 |

Jev only: 18/21 right.
