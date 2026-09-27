#![doc = include_str!("../README.md")]

mod definitions;
mod error;
mod gesture;
mod identity;
mod log;
mod pixels;
mod polygon;
mod cell_boxes;
mod degree;
mod predicate;
mod series;
mod resolve;
mod split;
mod state;
mod values;

pub use definitions::{ProducerDefinition, Projection, Resolution};
pub use error::{Error, Result};
pub use gesture::Gesture;
pub use log::{Drawn, LogEntry, Producers};
pub use identity::{ProducerId, ProjectionId, SelectionId, ViewId};
pub use pixels::PixelGrid;
pub use series::SeriesTest;
pub use resolve::{ConsumerFilter, EmptySelection, SelectionFilter, SelectionMode};
pub use split::{PredicateSplit, SelectionPredicates, SplitReason};
pub use state::{Contribution, SelectionSet, SelectionUpdate};
pub use values::{SelectionValue, ValueTest};
