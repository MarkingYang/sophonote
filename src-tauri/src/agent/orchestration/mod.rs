//! Bounded collaboration contracts and a controlled native Pi child path.
//! Product admission stays disabled pending consent, projection and recovery gates.
pub mod contracts;
pub mod evidence;
pub(crate) mod native_child;
pub mod scope;
pub mod sessions;
pub mod store;
pub mod supervisor;

pub(crate) mod admission;
pub mod projection;
