//! Per-stage semantic accounting that needs the session's meter: the charges,
//! diagnostics and working records each element family contributes during and
//! after scene resolution, kept beside the session kernel as additional
//! `impl RenderSession` blocks so each file states what it touches.

mod fallbacks;
mod text;
