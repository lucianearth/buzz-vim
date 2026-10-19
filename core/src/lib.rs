//! The UI-free half of buzz-vim, shared by the terminal client and the
//! iPhone app: relay IO, the live socket, the message store, read state,
//! Desktop's sections, agent telemetry, the color ramp, and the operator
//! loop's checklist records, Q pop-up and Needs-you tab, and push.

// Nested `if`s read better than let-chains here, as in the terminal app.
#![allow(clippy::collapsible_if, clippy::collapsible_match)]

pub mod checklist;
pub mod draft;
pub mod links;
pub mod live;
pub mod log;
pub mod mention;
pub mod model;
pub mod needs;
pub mod palette;
pub mod push;
pub mod quick;
pub mod readstate;
pub mod relay;
pub mod sections;
pub mod telemetry;
pub mod unread;
