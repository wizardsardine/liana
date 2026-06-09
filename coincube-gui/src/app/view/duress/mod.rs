//! Duress-mode UI surfaces.
//!
//! [`active_screen`] is the cryptic "Duress Mode Activated" dead-end that
//! replaces the entire app shell once duress is active (local-PIN wipe path or
//! remote activation). Its only interactive element is a Sign-in button that
//! gates entirely on server-side duress state (Phase 5).

pub mod active_screen;
