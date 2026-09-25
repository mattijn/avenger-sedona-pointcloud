//! A small chart layer on `avenger-scenegraph`, for one chart object that
//! transitions between states. `avenger-chart` has rects and circles only;
//! this layer adds bars, a pie, lines per series, a heatmap with a colour
//! scale and a map, with keyed transitions between them.

pub mod anim;
pub mod data;
pub mod draw;
pub mod editor;
pub mod model;
pub mod package;
pub mod pilot;
pub mod theme;
