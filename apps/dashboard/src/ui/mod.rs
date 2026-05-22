//! Dashboard primitive library. See `docs/design/dashboard-primitives.md`.
//!
//! Visual primitives are stateless wrappers over Tailwind utility classes;
//! the only behavior-bearing primitive is `ActionButton`, which binds to
//! `ProjectStore` via `MutationKey` so call sites do not repeat pending /
//! error wiring.

pub mod action_button;
pub mod card;
pub mod empty_state;
pub mod error_state;
pub mod loading_skeleton;
pub mod status_pill;
pub mod toast_region;

pub use action_button::{ActionButton, ButtonVariant};
pub use card::{Card, CardBody, CardFoot, CardHead};
pub use empty_state::{EmptyState, EmptyStateAccent};
pub use error_state::ErrorState;
pub use loading_skeleton::{SkeletonHeading, SkeletonLine};
pub use status_pill::{PillTone, StatusPill};
pub use toast_region::HudToastRegion;
