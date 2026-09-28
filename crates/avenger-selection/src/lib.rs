#![doc = include_str!("../README.md")]

mod cell_boxes;
mod definitions;
mod degree;
mod error;
mod gesture;
mod identity;
mod log;
mod pixels;
mod polygon;
mod predicate;
mod resolve;
mod series;
mod split;
mod state;
mod udf;
mod values;

pub use definitions::{ProducerDefinition, Projection, Resolution};
pub use error::{Error, Result};
pub use gesture::Gesture;
pub use identity::{ProducerId, ProjectionId, SelectionId, ViewId};
pub use log::{Drawn, LogEntry, Producers};
pub use pixels::PixelGrid;
pub use resolve::{ConsumerFilter, EmptySelection, SelectionFilter, SelectionMode};
pub use series::SeriesTest;
pub use split::{PredicateSplit, SelectionPredicates, SplitReason};
pub use state::{Contribution, SelectionSet, SelectionUpdate};
pub use values::{SelectionValue, ValueTest};
