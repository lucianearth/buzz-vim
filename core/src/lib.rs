//! The UI-free half of buzz-vim, shared by the terminal client and the
//! iPhone app: relay IO, the live socket, the message store, read state,
//! Desktop's sections, agent telemetry and the color ramp.

// Nested `if`s read better than let-chains here, as in the terminal app.
#![allow(clippy::collapsible_if, clippy::collapsible_match)]

pub mod draft;
pub mod links;
pub mod live;
pub mod log;
pub mod mention;
pub mod model;
pub mod palette;
pub mod readstate;
pub mod relay;
pub mod sections;
pub mod telemetry;
pub mod unread;
