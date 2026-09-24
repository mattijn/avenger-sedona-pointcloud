| Instruction | Decider | Decision after each word | Chart changes | … with threshold 0.6 | Right from word |
|---|---|---|---|---|---|
| make the bars red | rules | no_change → no_change → mark/bars → color/red | 2 | 2 | 4 of 4 |
| make the bars red | jev | highlight/outliers (0.46) → highlight/outliers (0.34) → highlight/outliers (0.33) → color/red (0.99) | 2 | 1 | 4 of 4 |
| make the bars red | llm | no_change → no_change → no_change → color/red | 1 | 1 | 4 of 4 |
| put the labels upright | rules | no_change → no_change → no_change → no_change | 0 | 0 | never of 4 |
| put the labels upright | jev | highlight/outliers (0.56) → highlight/outliers (0.43) → rotate_labels/keep (0.30) → rotate_labels/vertical (0.97) | 3 | 1 | 4 of 4 |
| put the labels upright | llm | no_change → no_change → rotate_labels/tilt → rotate_labels/vertical | 2 | 2 | 4 of 4 |
| the colour is too loud, something calmer please | rules | no_change → no_change → no_change → no_change → no_change → no_change → no_change → no_change | 0 | 0 | never of 8 |
| the colour is too loud, something calmer please | jev | highlight/outliers (0.51) → highlight/outliers (0.37) → highlight/outliers (0.37) → color/keep (0.77) → color/keep (0.97) → color/keep (0.97) → color/blue (0.99) → color/blue (0.99) | 2 | 1 | 7 of 8 |
| the colour is too loud, something calmer please | llm | no_change → color/blue → no_change → color/blue → color/grey → color/grey → color/grey → color/grey | 2 | 2 | 4 of 8 |
| zoom to the north-east | rules | no_change → no_change → no_change → zoom/north_east | 1 | 1 | 4 of 4 |
| zoom to the north-east | jev | zoom/keep (0.80) → zoom/keep (0.78) → zoom/keep (0.84) → zoom/north_east (0.99) | 1 | 1 | 4 of 4 |
| zoom to the north-east | llm | zoom/keep → zoom/keep → zoom/keep → zoom/north_east | 1 | 1 | 4 of 4 |
| can we look closer at the lower right part | rules | no_change → no_change → no_change → no_change → no_change → no_change → no_change → no_change → no_change | 0 | 0 | never of 9 |
| can we look closer at the lower right part | jev | mark/map (0.22) → mark/map (0.29) → zoom/keep (0.28) → zoom/keep (0.96) → zoom/keep (0.86) → zoom/keep (0.87) → zoom/south_west (0.96) → zoom/south_east (1.00) → zoom/south_east (1.00) | 3 | 2 | 8 of 9 |
| can we look closer at the lower right part | llm | no_change → no_change → no_change → zoom/keep → zoom/keep → zoom/keep → zoom/south_west → zoom/south_east → zoom/south_east | 2 | 2 | 8 of 9 |
| highlight the tallest buildings | rules | highlight/top_10 → highlight/top_10 → highlight/top_10 → highlight/top_10 | 1 | 1 | 1 of 4 |
| highlight the tallest buildings | jev | highlight/keep (0.57) → highlight/keep (0.60) → highlight/outliers (0.97) → highlight/top_10 (0.94) | 3 | 3 | 4 of 4 |
| highlight the tallest buildings | llm | highlight/top_10 → highlight/top_10 → highlight/outliers → highlight/top_10 | 3 | 3 | 4 of 4 |

| Decider | Chart changes | … with threshold 0.6 | Instructions ending right |
|---|---|---|---|
| jev | 14 | 9 | 6 of 6 |
| llm | 11 | 11 | 6 of 6 |
| rules | 4 | 4 | 3 of 6 |
