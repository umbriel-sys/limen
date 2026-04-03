//! Strongly-validated, zero-cost event message type.
//!
//! This module provides [`EventMessage`], a lightweight wrapper around a
//! `&'static str` that enforces strict formatting rules **at compile time**.
//! Messages must not exceed a fixed maximum byte length and must not contain
//! newline characters. Violations detected in a `const` context produce
//! compile-time errors.
//!
//! The accompanying [`event_message!`] macro guarantees that all validation
//! happens during compilation, even when invoked inside non-const functions.
//! When compiled successfully, an `EventMessage` is represented and used at
//! runtime with **no overhead**, making it as efficient as a bare `&'static str`.
//!
//! This module is intended for log messages, telemetry identifiers, and any
//! system where event message text must be validated, immutable, and fully
//! static while still incurring zero runtime cost.

/// A strongly-validated, zero-cost wrapper around a `&'static str` intended for
/// use in event messages, logs, telemetry, or any system where input strings
/// must meet strict formatting requirements.
///
/// # Overview
///
/// `EventMessage` wraps a `&'static str`, but with **compile-time validation**
/// whenever it is constructed in a `const` context. The validation enforces:
///
/// - The message must not exceed [`EventMessage::MAX_LEN`] bytes.
/// - The message must not contain newline characters (`'\n'` or `'\r'`).
///
/// When constructed using the accompanying [`event_message!`] macro, all
/// validation is **guaranteed to occur at compile time**, ensuring:
///
/// - **Zero runtime cost** (no loops, no checks, no panics).
/// - **Static enforcement** of message correctness.
/// - **Identical runtime performance** to using a plain `&'static str`.
///
/// # Why use this type?
///
/// This type is useful when you need a runtime message type that is:
///
/// - Guaranteed to be short enough for logging systems or wire protocols.
/// - Guaranteed to be newline-free (e.g., single-line logs).
/// - Immutable and `'static`.
/// - Just as cheap as storing a `&'static str`.
///
/// Once constructed, an `EventMessage` is simply a thin wrapper around a string
/// slice. Accessing the inner string via [`EventMessage::as_str`] is fully
/// inlined and has no measurable overhead.
///
/// # Compile-time vs runtime construction
///
/// - Using `EventMessage::new` inside a `const` context performs validation at
///   compile time.
/// - Using `EventMessage::new` at runtime may incur runtime checking cost.
/// - Using the [`event_message!`] macro **always** validates at compile time.
///
/// Prefer [`event_message!`] for maximal performance and strictness.
///
/// # Examples
///
/// ## Compile-time validated constant
///
/// ```rust
/// use limen_core::prelude::event_message::EventMessage;
///
/// const HELLO: EventMessage = EventMessage::new("hello");
/// ```
///
/// ## Preferable usage: enforced via `event_message!`
///
/// ```rust
/// use limen_core::prelude::event_message::EventMessage;
/// use limen_core::event_message;
///
/// let msg: EventMessage = event_message!("system_ready");
/// println!("{}", msg.as_str());
/// ```
///
/// Attempting to use invalid strings results in a **compile-time error**:
///
/// ```rust,compile_fail
/// use limen_core::prelude::event_message::EventMessage;
///
/// const BAD: EventMessage = EventMessage::new("line1\nline2"); // contains newline
/// ```
///
/// ```rust,compile_fail
/// use limen_core::prelude::event_message::EventMessage;
///
/// const TOO_LONG: EventMessage = EventMessage::new(
///     "12345678901234567890123456789012345678901234567890\
///12345678901234567890123456789012345678901234567890\
///123456789012345678901234567890"
/// );
/// ```
#[repr(transparent)]
#[derive(Copy, Clone, Debug)]
pub struct EventMessage(&'static str);

impl EventMessage {
    /// The maximum allowed message length in bytes.
    ///
    /// Messages longer than this value will cause a compile-time error if
    /// validated in a `const` context, or a panic if validated at runtime.
    pub const MAX_LEN: usize = 128;

    /// Creates a new `EventMessage` from a `&'static str`, performing
    /// full validation when invoked in a `const` context.
    ///
    /// # Validation
    ///
    /// This function checks:
    ///
    /// - The string's length does not exceed [`EventMessage::MAX_LEN`].
    /// - The string contains no newline characters (`'\n'` or `'\r'`).
    ///
    /// # Compile-Time Guarantees
    ///
    /// When invoked in a `const` context:
    ///
    /// - Validation happens during compilation.
    /// - Violations cause a compile-time error.
    /// - No runtime code is emitted for the check.
    ///
    /// # Runtime Behavior
    ///
    /// If invoked at runtime:
    ///
    /// - Validation executes at runtime.
    /// - Violations cause a runtime panic.
    ///
    /// For most usage, the [`event_message!`] macro ensures compile-time
    /// validation in all cases.
    pub const fn new(s: &'static str) -> Self {
        let bytes = s.as_bytes();
        let len = bytes.len();

        if len > Self::MAX_LEN {
            panic!("EventMessage is too long");
        }

        let mut index = 0;
        while index < len {
            let byte = bytes[index];
            if byte == b'\n' || byte == b'\r' {
                panic!("EventMessage contains newline characters");
            }
            index += 1;
        }

        Self(s)
    }

    /// Returns the inner `&'static str`.
    ///
    /// This method is `const` and `#[inline]`, and therefore has no runtime
    /// overhead. It is equivalent in performance to using a plain
    /// `&'static str` directly.
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

/// Constructs an [`EventMessage`] with **guaranteed compile-time validation**.
///
/// This macro ensures that:
///
/// - The message is validated in a `const` context.
/// - No runtime checks are ever emitted.
/// - The resulting value is as cheap as a `&'static str`.
///
/// # Why this macro is recommended
///
/// Even inside non-const functions, Rust evaluates the validation inside a
/// compile-time `const` item. This makes the macro strictly superior to calling
/// [`EventMessage::new`] directly at runtime.
///
/// # Examples
///
/// ```rust
/// use limen_core::prelude::event_message::EventMessage;
/// use limen_core::event_message;
///
/// let msg = event_message!("startup_complete");
/// println!("{}", msg.as_str());
/// ```
///
/// Invalid messages cause **compile-time errors**:
///
/// ```rust,compile_fail
/// use limen_core::prelude::event_message::EventMessage;
/// use limen_core::event_message;
///
/// let bad = event_message!("line1\nline2");
/// ```
///
/// ```rust,compile_fail
/// use limen_core::prelude::event_message::EventMessage;
/// use limen_core::event_message;
///
/// let too_long = event_message!(
///     "12345678901234567890123456789012345678901234567890\
///12345678901234567890123456789012345678901234567890\
///123456789012345678901234567890"
/// );
/// ```
#[macro_export]
macro_rules! event_message {
    ($s:expr) => {{
        const MSG: $crate::telemetry::event_message::EventMessage =
            $crate::telemetry::event_message::EventMessage::new($s);
        MSG
    }};
}
