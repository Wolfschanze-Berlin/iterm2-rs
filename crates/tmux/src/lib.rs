//! tmux control mode (-CC) protocol parser.
//! Parses the text-based protocol that tmux sends in control mode.

pub mod events;
pub mod parser;
pub mod session;

pub use events::TmuxEvent;
pub use parser::TmuxParser;
pub use session::{
    CommandType, PendingCommand, SessionManager, TmuxCommand, TmuxPane, TmuxSession, TmuxWindow,
};
