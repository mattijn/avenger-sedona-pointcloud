| Expression | SQL semantics | Vega semantics | Remaining differences, Vega semantics (row: Vega → here) |
|---|---|---|---|
| `datum.a + datum.b` | 6/8 | 8/8 |  |
| `datum.a / datum.b` | 6/8 | 8/8 |  |
| `datum.a % datum.b` | 6/8 | 8/8 |  |
| `-datum.a` | 7/8 | 8/8 |  |
| `datum.a * 2 + 1` | 7/8 | 8/8 |  |
| `datum.b / 0` | 7/8 | 8/8 |  |
| `datum.t / 1000` | 7/8 | 8/8 |  |
| `datum.s + datum.a` | 3/8 | 7/8 | 3: -3 → "null-3" |
| `'n=' + datum.a` | 3/8 | 8/8 |  |
| `datum.a + datum.s` | 3/8 | 7/8 | 3: -3 → "-3null" |
| `'v' + datum.f` | 7/8 | 8/8 |  |
| `datum.a == datum.b` | 6/8 | 8/8 |  |
| `datum.s == datum.a` | 2/8 | 8/8 |  |
| `datum.s === datum.a` | 8/8 | 8/8 |  |
| `datum.a < datum.b` | 6/8 | 8/8 |  |
| `datum.s < 'b'` | 7/8 | 8/8 |  |
| `datum.a ? 'yes' : 'no'` | 8/8 | 8/8 |  |
| `datum.s ? 'yes' : 'no'` | 8/8 | 8/8 |  |
| `datum.f ? 1 : 0` | 8/8 | 8/8 |  |
| `!datum.a` | 8/8 | 8/8 |  |
| `datum.a && datum.b` | 8/8 | 8/8 |  |
| `datum.a \|\| datum.b` | 8/8 | 8/8 |  |
| `datum.s \|\| 'empty'` | 8/8 | 8/8 |  |
| `isValid(datum.a)` | 8/8 | 8/8 |  |
| `isValid(datum.s)` | 8/8 | 8/8 |  |
| `isFinite(datum.a)` | 8/8 | 8/8 |  |
| `isNaN(datum.a)` | 7/8 | 8/8 |  |
| `toNumber(datum.s)` | 4/8 | 8/8 |  |
| `+datum.s` | 2/8 | 8/8 |  |
| `toString(datum.a)` | 4/8 | 8/8 |  |
| `toBoolean(datum.s)` | 5/8 | 8/8 |  |
| `round(datum.a)` | 6/8 | 8/8 |  |
| `floor(datum.a)` | 7/8 | 8/8 |  |
| `log(datum.a)` | 7/8 | 8/8 |  |
| `sqrt(datum.a)` | 7/8 | 8/8 |  |
| `pow(datum.a, 2)` | 7/8 | 8/8 |  |
| `max(datum.a, datum.b)` | 8/8 | 8/8 |  |
| `min(datum.a, datum.b)` | 5/8 | 8/8 |  |
| `clamp(datum.a, 0, 2)` | 6/8 | 8/8 |  |
| `abs(datum.a)` | 7/8 | 8/8 |  |
| `upper(datum.s)` | 7/8 | 8/8 |  |
| `length(datum.s)` | Vega throws | Vega throws | Vega throws: Cannot read properties of null (reading 'map') |
| `substring(datum.s, 0, 2)` | 7/8 | 8/8 |  |
| `indexof(datum.s, 'b')` | Vega throws | Vega throws | Vega throws: Cannot read properties of null (reading 'map') |
| `replace(datum.s, 'b', 'B')` | 7/8 | 8/8 |  |
| `if(datum.a > 1, 'big', 'small')` | 7/8 | 8/8 |  |
| `inrange(datum.a, [0, 2])` | 7/8 | 8/8 |  |
| `format(datum.a, ',.2f')` | 7/8 | 8/8 |  |
| `format(datum.a, '.1%')` | 7/8 | 8/8 |  |
| `timeFormat(datum.t, '%Y-%m-%d')` | 7/8 | 8/8 |  |
| `year(datum.t)` | 7/8 | 8/8 |  |
| `month(datum.t)` | 7/8 | 8/8 |  |

SQL semantics: 323/400 rows agree (80.8 %), 12 of 52 expressions agree on every row.

Vega semantics: 398/400 rows agree (99.5 %), 48 of 52 expressions agree on every row.
