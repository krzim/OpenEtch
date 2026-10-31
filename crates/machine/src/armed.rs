//! [`Armed`]: the token [`crate::Controller::run`] requires before anything can emit.

/// Proof that a person approved one run in the UI. [`crate::Controller::run`] takes it by value, so each approval
/// runs at most once.
///
/// **Only the engine's UI approval endpoint (`POST /api/runs/{id}/approve`) may call
/// [`Armed::approved_in_ui`].** API and MCP handlers must never construct one; they create pending runs. The type has
/// a private field, no `Default`, `Clone` or `Deserialize`, so it cannot be made any other way:
///
/// ```compile_fail,E0451
/// let armed = machine::Armed { run_id: "r1".into(), _private: () };
/// ```
///
/// and `run` cannot be called without one:
///
/// ```compile_fail,E0061
/// # use machine::{Controller, SimLink};
/// let mut c = Controller::new(SimLink::new());
/// let _ = c.run(&[]);
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct Armed {
    run_id: String,
    _private: (),
}

impl Armed {
    /// Arm run `run_id`. Call this only from the engine's UI approval endpoint (see the type docs).
    pub fn approved_in_ui(run_id: String) -> Self {
        Armed { run_id, _private: () }
    }

    /// The approved run.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
}
