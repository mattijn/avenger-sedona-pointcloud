//! Stage 1 check: one chart object through a scripted sequence of states,
//! rendered headlessly. Writes a contact sheet of mid-transition frames.
//!
//!     cargo run --release -p lidar-decide --bin layer_demo -- out/layer_demo

use avenger_wgpu::canvas::{Canvas, PngCanvas};
use lidar_decide::layer::anim::{still, transition};
use lidar_decide::layer::model::{resolve, Dataset, Mark, Quarter, State};
use lidar_decide::layer::{data, draw};

type Error = Box<dyn std::error::Error>;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let out = std::env::args().nth(1).unwrap_or("out/layer_demo".into());
    std::fs::create_dir_all(&out)?;
    let t = std::time::Instant::now();
    let d = data::load().await?;
    println!(
        "data {:.1?}: {} classes, {} flight bins, {} class×height cells, {} building cells",
        t.elapsed(), d.classes.len(), d.flight.len(), d.class_height.len(), d.cells.len()
    );
    let mut states = vec![State::new(Dataset::Classes)];
    let mut push = |f: &dyn Fn(&mut State)| {
        let mut s = states.last().unwrap().clone();
        f(&mut s);
        states.push(s);
    };
    push(&|s| s.color = Some([0.77, 0.31, 0.32, 1.0]));
    push(&|s| s.color = None);
    push(&|s| { s.mark = Mark::Pie; s.title = s.default_title(); });
    push(&|s| { s.mark = Mark::Bars; s.title = s.default_title(); });
    push(&|s| { s.dataset = Dataset::ClassHeight; s.mark = Mark::Heatmap; s.title = s.default_title(); });
    push(&|s| { s.dataset = Dataset::Flight; s.mark = Mark::Line; s.title = s.default_title(); });
    push(&|s| s.zoom = Some(Quarter::NorthWest));
    push(&|s| s.zoom = None);
    push(&|s| { s.dataset = Dataset::Cells; s.mark = Mark::Map; s.title = s.default_title(); });
    push(&|s| s.highlight = true);
    push(&|s| s.zoom = Some(Quarter::SouthEast));
    let frames: Vec<_> = states.iter().map(|s| resolve(s, &d)).collect();
    let mut canvas = PngCanvas::new(draw::dims(), Default::default()).await?;
    let mut n = 0;
    for w in frames.windows(2) {
        for t in [0.0, 0.25, 0.5, 0.75] {
            let scene = draw::scene(&transition(&w[0], &w[1], t));
            canvas.set_scene(&scene)?;
            canvas.render().await?.save(format!("{out}/f{n:03}.png"))?;
            n += 1;
        }
    }
    let scene = draw::scene(&still(frames.last().unwrap()));
    canvas.set_scene(&scene)?;
    canvas.render().await?.save(format!("{out}/f{n:03}.png"))?;
    println!("wrote {} frames to {out}", n + 1);
    Ok(())
}
