//! Reads the raw session blob each harness uploads into chat turns and tool calls.

mod canvas;
mod parse;

pub use canvas::Canvas;
pub use parse::{parse, Message, Parsed, Plan, Role, ToolCall};
