//! Environment variable lookup, injectable so tests never touch the process environment.

/// A source of environment variables.
pub trait Env {
    /// The value of `key`, or `None` when unset.
    fn var(&self, key: &str) -> Option<String>;
}

/// The real process environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessEnv;

impl Env for ProcessEnv {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// Any `Fn(&str) -> Option<String>` is an [`Env`], which keeps test doubles to one closure.
impl<F> Env for F
where
    F: Fn(&str) -> Option<String>,
{
    fn var(&self, key: &str) -> Option<String> {
        self(key)
    }
}
