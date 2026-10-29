//! The session state machine from ARCHITECTURE §8. Guards transitions so the app cannot
//! slip into an illegal state, and logs every move.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SessionState {
    /// Asleep in the tray; no overlay.
    #[default]
    Idle,
    /// Freezing the screen (taking the snapshot).
    Capturing,
    /// Overlay is up, no active selection yet.
    Selecting,
    /// A selection exists and is being edited.
    Editing,
    /// Producing output (Copy / Save / Save As).
    Exporting,
    /// Live video recording.
    Recording,
}

impl SessionState {
    fn allowed(self) -> &'static [SessionState] {
        use SessionState::*;
        match self {
            Idle => &[Capturing],
            Capturing => &[Selecting, Idle],
            Selecting => &[Editing, Exporting, Idle],
            Editing => &[Selecting, Exporting, Recording, Idle],
            Exporting => &[Idle],
            Recording => &[Idle],
        }
    }
}

type Listener = Box<dyn FnMut(SessionState, SessionState) + Send>;

#[derive(Default)]
pub struct SessionStateMachine {
    state: SessionState,
    listeners: Vec<Listener>,
}


impl SessionStateMachine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    /// Called after every successful transition with `(from, to)`.
    pub fn on_changed(&mut self, f: impl FnMut(SessionState, SessionState) + Send + 'static) {
        self.listeners.push(Box::new(f));
    }

    /// Whether `to` is reachable from the current state.
    pub fn can_transition(&self, to: SessionState) -> bool {
        self.state == to || self.state.allowed().contains(&to)
    }

    /// Attempts a transition; returns false (and logs) if it is not allowed.
    pub fn try_transition(&mut self, to: SessionState) -> bool {
        if self.state == to {
            return true;
        }
        if !self.can_transition(to) {
            log::warn!("Session: illegal transition {:?} → {:?} ignored.", self.state, to);
            return false;
        }
        let from = std::mem::replace(&mut self.state, to);
        log::info!("Session: {from:?} → {to:?}.");
        self.notify(from, to);
        true
    }

    /// Forces the session back to Idle (teardown).
    pub fn reset(&mut self) {
        if self.state == SessionState::Idle {
            return;
        }
        let from = std::mem::replace(&mut self.state, SessionState::Idle);
        log::info!("Session: {from:?} → Idle (reset).");
        self.notify(from, SessionState::Idle);
    }

    fn notify(&mut self, from: SessionState, to: SessionState) {
        for l in &mut self.listeners {
            l(from, to);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SessionState::*;
    use super::*;
    use std::sync::{Arc, Mutex};

    fn editing() -> SessionStateMachine {
        let mut sm = SessionStateMachine::new();
        for s in [Capturing, Selecting, Editing] {
            assert!(sm.try_transition(s));
        }
        sm
    }

    #[test]
    fn starts_idle() {
        assert_eq!(SessionStateMachine::new().state(), Idle);
    }

    #[test]
    fn walks_the_happy_path() {
        let mut sm = editing();
        assert!(sm.try_transition(Exporting));
        assert!(sm.try_transition(Idle));
        assert_eq!(sm.state(), Idle);
    }

    #[test]
    fn rejects_illegal_transition() {
        let mut sm = SessionStateMachine::new();
        assert!(!sm.try_transition(Editing));
        assert_eq!(sm.state(), Idle);
    }

    #[test]
    fn editing_toggles_back_to_selecting() {
        let mut sm = editing();
        assert!(sm.try_transition(Selecting));
        assert_eq!(sm.state(), Selecting);
    }

    #[test]
    fn editing_can_enter_recording() {
        let mut sm = editing();
        assert!(sm.try_transition(Recording));
        assert!(sm.try_transition(Idle));
    }

    #[test]
    fn reset_forces_idle_and_fires_change() {
        let mut sm = SessionStateMachine::new();
        sm.try_transition(Capturing);
        let seen = Arc::new(Mutex::new(None));
        let s = seen.clone();
        sm.on_changed(move |f, t| *s.lock().unwrap() = Some((f, t)));
        sm.reset();
        assert_eq!(sm.state(), Idle);
        assert_eq!(*seen.lock().unwrap(), Some((Capturing, Idle)));
    }

    #[test]
    fn same_state_transition_is_noop_true() {
        assert!(SessionStateMachine::new().try_transition(Idle));
    }
}
