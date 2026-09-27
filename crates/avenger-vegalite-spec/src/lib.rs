#![doc = include_str!("../README.md")]

mod bin;
mod data;
mod error;
mod presence;
#[cfg(feature = "schema")]
pub mod schema;
mod spec;
mod transform;
mod validate;

pub use bin::{Bin, BinOutput, BinParams};
pub use data::{Data, DataFormat, FormatType, InlineDataset};
pub use error::SpecError;
pub use presence::MissingNullOrValue;
pub use spec::{
    Axis, BarMark, Camera, Config, Encoding, FieldType, Mark, MarkType, Orient, Parameter,
    PositionFieldDef, Scale, ScaleType, SecondaryFieldDef, SortOrder, StackOffset, Text, UnitSpec,
    ViewConfig,
};
pub use transform::{
    AggregateOp, AggregateTransform, AggregatedFieldDef, BinTransform, ExpressionReference,
    FieldPredicate, FilterTransform, PredicateOperand, Transform,
};
