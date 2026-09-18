# browser-wpt-results

Per-area [Web Platform Tests](https://web-platform-tests.org) score summaries
for browsers (Chrome, Firefox, Safari, Servo, ...) derived from
[wpt.fyi](https://wpt.fyi) data, in the same format as
[blitz-wpt-results](https://github.com/DioxusLabs/blitz-wpt-results): one
compact time series per WPT directory ("area").

## Data sources

Two sources, both derived from the same wptreports so they score identically:

- **results-analysis-cache** (historical backfill): a local clone of
  [web-platform-tests/results-analysis-cache](https://github.com/web-platform-tests/results-analysis-cache),
  where every wpt.fyi run is an orphan commit tagged `run/<id>/results` whose
  tree mirrors WPT with one JSON blob per test (`status` + `subtests[].status`).
  One ~2 GB clone covers every run since mid-2017 and is read with `gix` at
  ~1 s per run for the whole WPT tree, so no per-run downloads are needed.
- **wpt.fyi summary files** (incremental): the run's `summary_v2.json.gz`
  (~1 MB), fetched only for runs the cache doesn't have yet (the cache is
  refreshed every 3 hours) or for products it doesn't track (e.g. Ladybird).
  Requests are sequential with a 1 s pause between them (`--fetch-delay`).
  Pre-July-2022 summaries are in the v1 format, which folds the harness status
  into the subtest counts, so prefer the cache for anything historical.

## Usage

```sh
git clone --bare https://github.com/web-platform-tests/results-analysis-cache.git ../results-analysis-cache.git

# Backfill, one run per browser per day
cargo run -r -- --cache ../results-analysis-cache.git --daily

# Incremental update: only runs not already in summary/<product>/runs.json are
# scored; runs missing from the cache fall back to wpt.fyi summaries
cargo run -r -- --cache ../results-analysis-cache.git --daily --from 2026-09-01

# A product that isn't in the cache
cargo run -r -- --source summary --product ladybird --daily --from 2026-09-01

# Re-score runs that are already present (e.g. after a scoring change)
cargo run -r -- --cache ../results-analysis-cache.git --daily --from 2026-09-01 --rescore

cargo run -r -- --help
```

Defaults: products `chrome firefox safari servo`, runs labelled
`master,experimental`, output in `./summary`. Tests with status `SKIP` are
dropped before scoring, as for Blitz. Runs are de-duplicated by wpt.fyi run ID.

## Data format

One dataset per product:

```text
summary/
  chrome/
    runs.json                 # shared per-run metadata (one entry per run)
    total.json                # scores for the whole run (sum of all top-level areas)
    areas/
      css.json                # scores for the whole "css" suite
      css/
        css-flexbox.json      # scores for "css/css-flexbox"
        css-flexbox/
          alignment.json      # scores for "css/css-flexbox/alignment"
      html.json
      ...
  firefox/
    ...
```

### `runs.json`

```json
{
"runs":[
{"date":"2026-09-01T00:38:38Z","wpt_revision":"3e1155aec","product_revision":"154.0.8035.0","run_id":5138501145460736}
]}
```

| Field | Meaning |
| --- | --- |
| `date` | Start time of the wpt.fyi run (RFC 3339). |
| `wpt_revision` | Revision of the WPT test suite that was run (9-char sha). |
| `product_revision` | Browser version reported by wpt.fyi. |
| `run_id` | wpt.fyi run ID. |

Runs are sorted by `(date, product_revision)`.

### Area files (`areas/<area path>.json`, `total.json`)

Each area file contains a single `scores` array with **one entry per run,
index-aligned with `runs.json`** (entry *i* belongs to run *i*). `total.json`
has the same shape and holds the whole-run total (every top-level area
summed); it lives next to `runs.json` so it can't collide with a WPT
directory, and is only written when the whole tree was scored (no
`--subtree`):

```json
{
"scores":[
[23972,8658.8,33098,10907],
null,
[23980,8672.8,33112,10921]
]}
```

Each entry is either `null` (the area has no data for that run) or a 4-tuple:

```text
[total_tests, total_score, total_subtests, total_subtests_passed]
```

| Index | Meaning |
| --- | --- |
| 0 | Number of tests run in this area |
| 1 | Servo-style score (sum over tests of the fraction of subtests passed), rounded to 1 decimal place |
| 2 | Number of subtests run |
| 3 | Number of subtests passed |

A test without subtests (reftests, crashtests) counts as one subtest that
passes iff the test status is `PASS`.
