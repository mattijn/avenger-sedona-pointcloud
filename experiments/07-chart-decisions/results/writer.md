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
| w03 only emphasise buildings taller than 70 m | **✗** map top 10 % (threshold) | map >= 70 | map >= 70 | map >= 70 |
| w04 use 10 m cells instead of 5 m | **✗** unchanged (rows) | map 3489 cells | map 3489 cells | map 3489 cells |
| w05 zoom to easting 657200 to 657600 and northing 6867300 to 6867700 | **✗** map zoom north_east (range) | map zoom custom | map zoom custom | map zoom custom |
| w06 zoom to the first 5 seconds | **✗** line zoom north_west (range_x) | line zoom custom | line zoom custom | line zoom custom |
| w07 make them orange and call it Point counts | **✗** bars orange (title) | bars "Point counts" orange | bars "Point counts" orange | bars "Point counts" orange |
| w08 back to bars in red, titled Classes | **✗** bars (colour, title) | bars "Classes" red | bars "Classes" red | bars "Classes" red |
| w09 colour the map green and emphasise everything above 60 m | **✗** map green (highlight, threshold) | map green >= 60 | map green >= 60 | map green >= 60 |
| w10 show the buildings in 2 m cells | **✗** unchanged (rows_gt) | map 63231 cells | map 63231 cells | map 63231 cells |
| w11 zoom to the north-east and emphasise buildings over 65 m | **✗** map zoom north_east (highlight, threshold) | map zoom north_east >= 65 | map zoom north_east >= 65 | map zoom north_east >= 65 |
| w12 draw the buildings as a 3D model | unchanged | unchanged | unchanged | unchanged |
| w13 emphasise the band with the most points | unchanged | unchanged, 2 tries | unchanged, 2 tries | unchanged, 2 tries |
| w14 filter ground | **✗** unchanged (rows) | pie 1 cells | pie 1 cells | pie 1 cells |
| w15 only show ground and buildings | **✗** unchanged (rows) | bars 2 cells | bars 2 cells | bars 2 cells |

| Arm | Right, vocabulary (v) | Right, own text (w) | Writer used | Accepted on the first try | Tries | Latency p50 / max | Cost |
|---|---|---|---|---|---|---|---|
| jev | 6/6 | 2/15 | 0 | – | 0 | 296 / 677 ms | $0.0011 |
| haiku | 6/6 | 15/15 | 21 | 20/21 | 22 | 1681 / 3343 ms | $0.0603 |
| haiku+jev | 6/6 | 15/15 | 21 | 20/21 | 22 | 1952 / 3971 ms | $0.0626 |
| routed | 6/6 | 15/15 | 16 | 15/16 | 17 | 1618 / 3971 ms | $0.0480 |

| Case | Start | Expected | Jev | Confidence |
|---|---|---|---|---|
| b01 undo | bars | undo | undo | 0.90 |
| b02 undo that | map | undo | undo | 0.88 |
| b03 go back | pie | undo | undo | 0.86 |
| b04 that was wrong, revert it | map | undo | undo | 0.94 |
| b05 maak dat ongedaan | bars | undo | undo | 0.91 |
| b06 terug naar hoe het was | line | undo | undo | 0.62 |
| b07 start over | map | reset | reset | 0.95 |
| b08 reset everything | heatmap | reset | reset | 0.96 |
| b09 begin opnieuw | pie | reset | reset | 0.89 |
| b10 reset the zoom | map | zoom | zoom/all | 0.80 |
| b11 remove the emphasis | map | highlight | highlight/none | 1.00 |
| b12 leave it as it was | bars | no_change | no_change | 0.99 |
| b13 go back to the bar chart | line | mark | mark/bars | 0.87 |

Undo and reset: 13/13 right.
