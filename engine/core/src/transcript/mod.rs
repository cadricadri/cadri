//! Reads the raw session blob each harness uploads into chat turns and tool calls.

mod parse;

pub use parse::{parse, Message, Parsed, Role, ToolCall};
