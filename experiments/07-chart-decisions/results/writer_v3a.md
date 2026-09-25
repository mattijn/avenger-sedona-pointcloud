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
| w12 draw the buildings as a 3D model | unchanged | unchanged | unchanged | unchanged |
| w13 emphasise the band with the most points | unchanged | unchanged, 2 tries | unchanged, 2 tries | unchanged, 2 tries |
| w14 filter ground | **✗** unchanged (rows) | pie 1 cells | **✗** bars 1 cells (mark) | **✗** bars 1 cells (mark) |
| w15 only show ground and buildings | **✗** unchanged (rows) | bars 2 cells | bars 2 cells, 2 tries | bars 2 cells, 2 tries |

| Arm | Right, vocabulary (v) | Right, own text (w) | Writer used | Accepted on the first try | Tries | Latency p50 / max | Cost |
|---|---|---|---|---|---|---|---|
| jev | 6/6 | 2/15 | 0 | – | 0 | 279 / 498 ms | $0.0011 |
| haiku | 6/6 | 15/15 | 21 | 20/21 | 22 | 1681 / 3343 ms | $0.0603 |
| haiku+jev | 6/6 | 14/15 | 21 | 19/21 | 23 | 1817 / 3975 ms | $0.0661 |
| routed | 6/6 | 14/15 | 16 | 14/16 | 18 | 1675 / 3975 ms | $0.0514 |
