| Case | jev | haiku | haiku+jev | routed |
|---|---|---|---|---|
| v01 make the bars red | bars red | bars red | bars red | bars red |
| v02 which share does each class have? | pie | pie | pie | pie |
| v03 zoom in on the start of the lines, the top left | line zoom north_west | **✗** line zoom custom (zoom) | **✗** line zoom custom (zoom) | line zoom north_west |
| v04 mark the tallest buildings | map top 10 % | map top 10 % | map top 10 % | map top 10 % |
| v05 kleur de gebouwen groen | map green | map green, 2 tries | map green, 2 tries | map green, 2 tries |
| v06 nice, leave it like this | unchanged | unchanged | unchanged | unchanged |
| w01 call it Rooftops of the tile | **✗** unchanged (title) | map "Rooftops of the tile" | map "Rooftops of the tile" | map "Rooftops of the tile" |
| w02 noem de grafiek Punten per klasse | **✗** unchanged (title) | bars "Punten per klasse" | bars "Punten per klasse" | bars "Punten per klasse" |
| w03 only emphasise buildings taller than 70 m | **✗** map top 10 % (threshold) | map >= 70 | map >= 70 | map >= 70 |
| w04 use 10 m cells instead of 5 m | **✗** unchanged (rows) | map 3489 cells | map "Buildings, 10 m cells" 3489 cells | map "Buildings, 10 m cells" 3489 cells |
| w05 zoom to easting 657200 to 657600 and northing 6867300 to 6867700 | **✗** map zoom north_east (range) | map zoom custom | map zoom custom | map zoom custom |
| w06 zoom to the first 5 seconds | **✗** line zoom north_west (range_x) | line zoom custom | line zoom custom | line zoom custom |
| w07 make them orange and call it Point counts | **✗** bars orange (title) | bars "Point counts" orange, 2 tries | bars "Point counts" orange, 2 tries | bars "Point counts" orange, 2 tries |
| w08 back to bars in red, titled Classes | **✗** bars (colour, title) | bars "Classes" red, 2 tries | bars "Classes" red, 2 tries | bars "Classes" red, 2 tries |
| w09 colour the map green and emphasise everything above 60 m | **✗** map green (highlight, threshold) | map green >= 60, 2 tries | map green >= 60, 2 tries | map green >= 60, 2 tries |
| w10 show the buildings in 2 m cells | **✗** unchanged (rows_gt) | map 63231 cells | map "Buildings, 2 m cells" 63231 cells | map "Buildings, 2 m cells" 63231 cells |
| w11 zoom to the north-east and emphasise buildings over 65 m | **✗** map zoom north_east (highlight, threshold) | map zoom north_east >= 65 | map zoom north_east >= 65 | map zoom north_east >= 65 |
| w12 draw the buildings as a 3D model | unchanged | unchanged | unchanged | unchanged |
| w13 emphasise the band with the most points | unchanged | unchanged, 3 tries | unchanged, 3 tries | unchanged, 3 tries |

| Arm | Right, vocabulary (v) | Right, own text (w) | Writer used | Accepted on the first try | Tries | Latency p50 / max | Cost |
|---|---|---|---|---|---|---|---|
| jev | 6/6 | 2/13 | 0 | – | 0 | 271 / 1003 ms | $0.0010 |
| haiku | 5/6 | 13/13 | 19 | 14/19 | 25 | 2093 / 10679 ms | $0.0689 |
| haiku+jev | 5/6 | 13/13 | 19 | 14/19 | 25 | 2278 / 8462 ms | $0.0713 |
| routed | 6/6 | 13/13 | 15 | 10/15 | 21 | 1933 / 8462 ms | $0.0593 |
