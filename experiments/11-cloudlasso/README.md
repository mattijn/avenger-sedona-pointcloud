# Experiment 11: CloudLasso through avenger-selection

CloudLasso (Yu et al. 2012) selects in three dimensions from a lasso drawn in
two: of the points whose projection falls inside the lasso, it keeps the
largest connected region of dense voxels. Experiment 7 draws it with its own
code; the probe measures it through `avenger-selection` (FINDINGS.md 28) but
prints numbers only. This experiment draws the crate's result.

The setting is the probe's: a 500 m window of the tile, experiment 7's tilted
view (yaw 30°, elevation 35°) on a 400 px plot, its lasso, voxels 1/40 of the
window across and 1/10 of its height range up, and a structure of 0.3. The
chart and the crate split the work as the finding describes: the lasso is a
`SelectionValue::polygon` on 1 px screen cells; the chart counts the lasso's
points per voxel through the crate's predicate and finds the largest dense
region (experiment 7's algorithm, `lidar_common::largest_region`); the crate
then selects that region's voxels as `SelectionValue::cells`, intersected with
the lasso in one selection. The figure reads both memberships back through
`ConsumerFilter::predicate`.

## Run

```sh
cargo run --release -p lidar-cloudlasso --bin cloudlasso -- data/LHD_FXX_0657_6868_PTS_O_LAMB93_IGN69.copc.laz
```

Without a GPU, see experiment 10's README for the software Vulkan driver.

## The figure

![CloudLasso](images/cloudlasso.png)

In the tilted view the lasso covers a block of buildings. From above, the
lasso's points (orange) include low ground and structures in front of and
inside the block, which the lasso's prism passes through; CloudLasso (blue)
keeps the connected mass of the block itself. From the side, across the
tilted view, the orange that remains lies above the block's main heights and
on the ground beneath it. The classes of those points were not checked.

## Measured

In this cloud container (4 cores), on 4,200,019 points in the window:

| | |
|---|---|
| points in the lasso | 1,097,131 (experiment 7's Rust: 1,097,132) |
| CloudLasso | the largest of 8 dense regions, 422 voxels, 721,463 points |
| experiment 7's Rust | the largest of 8 regions, 421 voxels, 720,791 points, 0.09 % apart |
| lasso, voxel counts, region and voxel value | 124–133 ms |
| both memberships of every point, read back | 118–143 ms |

The small difference is finding 28's: the crate's cell kernel places voxel
edges in Float32, experiment 7's in Float64, so a few points on an edge fall
into the neighbouring voxel.
