# Working on this repo

This repo is two things at once:

1. **Experiments** that draw a real LiDAR point cloud with Avenger, queried
   from `sedona-pointcloud` on DataFusion.
2. **A running review of Avenger**, which is under heavy development by
   [@jonmmease](https://github.com/jonmmease). [FINDINGS.md](FINDINGS.md) is the
   ledger we hand back to him.

The work is a cycle, not a one-off: Jon pushes commits, we repin, rerun, and
report what changed. The most valuable output is a *measured* finding, not an
impression.

## The cycle, when Avenger has moved

### 1. See what moved

The Avenger checkout usually sits at `../avenger`. Clone it there if missing.

```sh
git -C ../avenger fetch --all --prune
gh pr list --repo jonmmease/avenger --state open --limit 20 \
  --json number,title,headRefName,baseRefName,updatedAt \
  --template '{{range .}}{{.number}} {{.headRefName}} -> {{.baseRefName}} | {{.title}}{{"\n"}}{{end}}'
```

The core work lands as a *stack* of PRs, each based on the previous one. We pin
the **top of the stack** — the branch nothing else is based on. `main` moves
rarely and lags far behind; do not pin it without a reason.

To read code on a branch without checking it out, use literal refs:
`git show 'origin/codex/selection:avenger-guides/src/legend/colorbar.rs'`.
(In zsh, `"$B:avenger-…"` silently mangles the path — `:a` is a path modifier.)

### 2. Repin

One line, in `[workspace.dependencies]` of the root `Cargo.toml`: every
`avenger-*` entry shares the same `rev`.

```sh
NEW=$(git -C ../avenger rev-parse origin/codex/<top-of-stack>)
sed -i '' "s/<old-rev>/$NEW/g" Cargo.toml
CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback cargo update
cargo build --release --workspace
```

The env var is needed because this machine runs Rust 1.89 while the newest
`ordered-float` wants 1.90. If the build breaks, that itself is a finding:
record what broke and which API changed.

### 3. Rerun everything

```sh
T=data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz
./scripts/download_tile.sh                                   # if data/ is empty

cargo run --release -p lidar-charts --bin cross_section -- $T out/cross_section.png
cargo run --release -p lidar-charts --bin topviews      -- $T out
cargo run --release -p lidar-charts --bin bench_window  -- $T
cargo run --release -p lidar-charts --bin explorer      -- $T --snapshots out
cargo run --release -p lidar-probes --bin probe_guides
cargo run --release -p lidar-probes --bin probe_render
cargo run --release -p lidar-coords --bin coords -- $T experiments/05-coordinate-systems/images
cargo run --release -p lidar-coords --bin coords_live -- $T --snapshots out/coords_live
cargo run --release -p lidar-pipeline --bin pipeline_bench -- $T
cargo run --release -p lidar-pipeline --bin cqrs_check -- $T out
cargo run --release -p avenger-validate -- corpus experiments/07-chart-decisions/results 20
cargo test --release -p avenger-validate

cargo run --release -p lidar-stream --bin stream_server -- $T          # terminal 1
cargo run --release -p lidar-stream --bin stream_live -- --speed 8 --window 12 --snapshots out
```

Compare against the tables in [FINDINGS.md](FINDINGS.md) and the timings in
[README.md](README.md). Renders should be visually identical to the committed
images in each experiment's `images/`; compare at equal size, since committed
copies are downscaled and dense point images differ under resampling.

### 4. Write it down

Update [FINDINGS.md](FINDINGS.md): the revision and date at the top, the status
of each entry, and any new finding *with the command that measures it*. Keep
resolved entries, marked fixed — the history of what got better is the point.

Then update the README timings if they moved, and commit.

## How to be useful here

- **Measure the pieces separately before blaming one.** This repo once reported
  that frame cost was rendering; timing `set_scene`, `render` and the geometry
  index apart showed rendering was 7% of it and the hit-test index was the rest.
  A wrong attribution wastes Jon's time, which is the one thing this repo is
  meant to save.
- **Prefer a probe to a paragraph.** If a finding cannot be re-measured by a
  command, it will not survive the next round. Both probes live in `probes/` and
  print their evidence; extend them rather than writing one-off scripts.
- **Verify before claiming.** Run the thing. "Builds clean" means the build was
  run; "still broken on #124" means the probe was run against #124.
- **Say what you did not check.** Unverified items belong in FINDINGS.md marked
  as such, not omitted.
- **Ask before anything leaves the machine.** Never open an issue, comment on a
  PR, or push to a repo other than this one without the user asking for it.
  Drafting the text for them to read is welcome.

## Adding an experiment

Each experiment is a crate under `experiments/NN-name/` with its own README and
`images/`. To add one:

1. `experiments/04-<name>/Cargo.toml` — name it `lidar-<short>`, take deps from
   `[workspace.dependencies]` with `dep.workspace = true`, and depend on
   `lidar-common` for the LAS session and palette.
2. Add it to `members` in the root `Cargo.toml`.
3. Write `experiments/04-<name>/README.md`: what question it asks, how to run
   it, what was measured.
4. Add a row to the table in the root README.

Shared code goes in `common/` only when a second crate needs it; a helper used
once stays in its binary.

## Gotchas learned the hard way

- **Disk.** This machine has run out of space mid-build twice. Check
  `df -h /System/Volumes/Data` first; a workspace build wants a few GB and
  `target/` grows past 3 GB. Build caches (`~/.cache/uv`, `~/Library/Caches/rattler`,
  npm, pip) are the safe things to clear, with the user's agreement.
- **Port 50051** stays held by a leftover `stream_server`. Check with
  `lsof -nP -iTCP:50051 -sTCP:LISTEN`, kill only your own, or pass another
  address: `stream_server <tile> 127.0.0.1:50060` and
  `stream_live --addr http://127.0.0.1:50060`.
- **Don't screenshot a background window for evidence.** macOS stops presenting
  frames for fully occluded windows, so a capture can be silently stale — two
  identical "frames" once looked like a hang that wasn't. Use `--snapshots` or
  `--record`, which render the same scene headlessly.
- **A new viewer window takes keyboard focus**, so keys meant for an older
  window go to it. Don't leave stray viewers running.
- **Capturing one window** (when it is genuinely wanted): get the id with
  JavaScript for Automation, then `screencapture -x -o -l <id> out.png`.

  ```sh
  osascript -l JavaScript -e 'ObjC.import("CoreGraphics");
    const l = ObjC.castRefToObject($.CGWindowListCopyWindowInfo($.kCGWindowListOptionAll, $.kCGNullWindowID));
    const o=[]; for (let i=0;i<l.count;i++){const w=l.objectAtIndex(i);
    const n=ObjC.unwrap(w.objectForKey("kCGWindowName"))||"";
    if(n.indexOf("LiDAR")>=0) o.push(ObjC.unwrap(w.objectForKey("kCGWindowNumber")));} o.join(",")'
  ```

- **Synthetic keystrokes** via `osascript` need Accessibility permission, which
  is usually not granted. Drive the app through its flags instead, or ask the
  user to press the key.
- **The tile** in `data/` may be a symlink into a scratch directory from an
  earlier session. If it is missing, `./scripts/download_tile.sh` fetches it
  again (~105 MB). Neither the tile nor the exported Parquet is in git.
- **The chart-language branch** (`jonmmease/facet-fresh-start`) does not build
  as cloned: `avenger-typst-label` points at a local `../../typst` checkout.
  See `experiments/02-chart-language/README.md` for the edit.

## Conventions

- Commit messages: a short imperative subject, then what changed and why, in
  plain prose. End with the Co-Authored-By line the session is given.
- Keep the README's numbers true. If a measurement changes, change the table.
- British spelling, sentences over bullet fragments where it reads better, and
  no adjectives a measurement doesn't back.
